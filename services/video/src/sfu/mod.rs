pub mod session;
pub mod udp;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use str0m::change::SdpOffer;
use str0m::media::{Direction, KeyframeRequestKind, MediaData, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::signaling::ServerMessage;

pub use session::{SfuParticipant, SfuSession, TrackIn, TrackOut, TrackOutState};

pub type ParticipantId = Uuid;
pub type SessionId = Uuid;

/// Command sent from WebSocket handler to the SFU loop.
#[derive(Debug)]
pub enum SfuCommand {
    /// New participant with SDP offer
    Join {
        session_id: SessionId,
        participant_id: ParticipantId,
        user_id: String,
        sdp_offer: String,
        reply_tx: mpsc::UnboundedSender<ServerMessage>,
    },
    /// SDP answer from client (renegotiation response)
    Answer {
        session_id: SessionId,
        participant_id: ParticipantId,
        sdp_answer: String,
    },
    /// ICE candidate from client
    IceCandidate {
        session_id: SessionId,
        participant_id: ParticipantId,
        candidate: String,
        sdp_mid: Option<String>,
    },
    /// Participant leaving
    Leave {
        session_id: SessionId,
        participant_id: ParticipantId,
    },
}

/// The SFU engine. Runs in a single tokio task, owns all Rtc instances.
pub struct SfuEngine {
    sessions: HashMap<SessionId, SfuSession>,
    udp_socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    /// The public-facing candidate address (real IP + UDP port)
    candidate_addr: SocketAddr,
    cmd_rx: mpsc::UnboundedReceiver<SfuCommand>,
    /// Media forwarding stats (logged periodically)
    stats_audio_fwd: u64,
    stats_video_fwd: u64,
    stats_last_log: Instant,
}

impl SfuEngine {
    pub fn new(
        udp_socket: Arc<UdpSocket>,
        public_ip: std::net::IpAddr,
        cmd_rx: mpsc::UnboundedReceiver<SfuCommand>,
    ) -> Self {
        let local_addr = udp_socket.local_addr().expect("UDP local addr");
        let candidate_addr = SocketAddr::new(public_ip, local_addr.port());
        Self {
            sessions: HashMap::new(),
            udp_socket,
            local_addr,
            candidate_addr,
            cmd_rx,
            stats_audio_fwd: 0,
            stats_video_fwd: 0,
            stats_last_log: Instant::now(),
        }
    }

    /// Main SFU event loop. Call this from a spawned tokio task.
    pub async fn run(mut self) {
        let mut buf = vec![0u8; 2000];
        // 20ms tick aligns with Opus audio frame rate (50 frames/sec).
        // Lower = less jitter for audio forwarding, more CPU.
        let mut interval = tokio::time::interval(Duration::from_millis(20));

        tracing::info!(local_addr = %self.local_addr, "SFU engine started");

        loop {
            tokio::select! {
                // 1. Incoming UDP packets
                result = self.udp_socket.recv_from(&mut buf) => {
                    match result {
                        Ok((n, source)) => {
                            self.handle_udp_packet(&buf[..n], source);
                        }
                        Err(e) => {
                            tracing::error!("UDP recv error: {e}");
                        }
                    }
                }

                // 2. Commands from WebSocket handlers
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(cmd) => {
                            self.handle_command(cmd);
                            // Drain all pending commands before polling
                            // (ensures ICE candidates are added before timeout fires)
                            while let Ok(cmd) = self.cmd_rx.try_recv() {
                                self.handle_command(cmd);
                            }
                        }
                        None => {
                            tracing::info!("SFU command channel closed, shutting down");
                            break;
                        }
                    }
                }

                // 3. Timer tick -- drive all Rtc instances forward
                _ = interval.tick() => {
                    self.tick();

                    // Log forwarding stats every 5 seconds
                    if self.stats_last_log.elapsed() >= Duration::from_secs(5) {
                        if self.stats_audio_fwd > 0 || self.stats_video_fwd > 0 {
                            let elapsed = self.stats_last_log.elapsed().as_secs_f32();
                            tracing::info!(
                                audio_pps = (self.stats_audio_fwd as f32 / elapsed) as u32,
                                video_pps = (self.stats_video_fwd as f32 / elapsed) as u32,
                                "media forwarding stats"
                            );
                        }
                        self.stats_audio_fwd = 0;
                        self.stats_video_fwd = 0;
                        self.stats_last_log = Instant::now();
                    }
                }
            }

            // After any event, poll all Rtc instances for outputs
            self.poll_all_outputs();
        }
    }

    fn handle_command(&mut self, cmd: SfuCommand) {
        match cmd {
            SfuCommand::Join {
                session_id,
                participant_id,
                user_id,
                sdp_offer,
                reply_tx,
            } => {
                self.handle_join(session_id, participant_id, user_id, sdp_offer, reply_tx);
            }
            SfuCommand::Answer {
                session_id,
                participant_id,
                sdp_answer,
            } => {
                self.handle_answer(session_id, participant_id, sdp_answer);
            }
            SfuCommand::IceCandidate {
                session_id,
                participant_id,
                candidate,
                sdp_mid,
            } => {
                self.handle_ice_candidate(session_id, participant_id, candidate, sdp_mid);
            }
            SfuCommand::Leave {
                session_id,
                participant_id,
            } => {
                self.handle_leave(session_id, participant_id);
            }
        }
    }

    fn handle_join(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
        user_id: String,
        sdp_offer: String,
        reply_tx: mpsc::UnboundedSender<ServerMessage>,
    ) {
        // Parse the SDP offer (try JSON first, then raw SDP string)
        let offer: SdpOffer = match serde_json::from_str(&sdp_offer) {
            Ok(o) => o,
            Err(_) => match SdpOffer::from_sdp_string(&sdp_offer) {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!(%participant_id, "invalid SDP offer: {e}");
                    let _ = reply_tx.send(ServerMessage::Error {
                        message: format!("invalid SDP offer: {e}"),
                    });
                    return;
                }
            },
        };

        // Create Rtc instance
        // Note: full ICE (not ICE-lite). ICE-lite causes immediate Disconnected
        // because str0m expects STUN before poll_output runs the first timeout.
        let mut rtc = Rtc::new();

        // Add local candidate (public IP + UDP port)
        let candidate = Candidate::host(self.candidate_addr, "udp").expect("host candidate");
        rtc.add_local_candidate(candidate);

        // Accept the offer and generate answer
        let answer = match rtc.sdp_api().accept_offer(offer) {
            Ok(answer) => answer,
            Err(e) => {
                tracing::warn!(%participant_id, "failed to accept SDP offer: {e}");
                let _ = reply_tx.send(ServerMessage::Error {
                    message: format!("SDP negotiation failed: {e}"),
                });
                return;
            }
        };

        // Convert to raw SDP string (not JSON-wrapped)
        let answer_str = answer.to_sdp_string();

        // Get or create session
        let session = self
            .sessions
            .entry(session_id)
            .or_insert_with(|| SfuSession::new(session_id));

        // Before adding new participant, collect existing incoming tracks
        // so we can set up forwarding
        let existing_tracks: Vec<(ParticipantId, Mid, MediaKind)> = session
            .participants
            .values()
            .flat_map(|p| p.tracks_in.iter().map(|t| (p.id, t.mid, t.kind)))
            .collect();

        // Add participant
        let participant = SfuParticipant {
            id: participant_id,
            user_id,
            rtc,
            ws_tx: reply_tx.clone(),
            tracks_in: Vec::new(),
            tracks_out: Vec::new(),
            pending_offer: None,
            last_activity_at: Instant::now(),
            ice_disconnected: false,
        };
        session.participants.insert(participant_id, participant);

        // Send SDP answer
        let _ = reply_tx.send(ServerMessage::Answer {
            sdp_answer: answer_str,
            participant_id,
        });

        // Set up track forwarding: new participant needs outgoing tracks
        // for all existing participants' incoming tracks
        if !existing_tracks.is_empty() {
            let new_participant = session.participants.get_mut(&participant_id).unwrap();
            for (origin_pid, mid, kind) in &existing_tracks {
                new_participant.tracks_out.push(TrackOut {
                    origin: *origin_pid,
                    origin_mid: *mid,
                    kind: *kind,
                    state: TrackOutState::ToOpen,
                });
            }
            tracing::info!(
                %participant_id,
                existing = existing_tracks.len(),
                "queued existing tracks for new participant"
            );
        }

        tracing::info!(%session_id, %participant_id, "participant joined SFU");
    }

    fn handle_answer(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
        sdp_answer: String,
    ) {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return;
        };
        let Some(participant) = session.participants.get_mut(&participant_id) else {
            return;
        };

        let answer = match serde_json::from_str(&sdp_answer)
            .or_else(|_| str0m::change::SdpAnswer::from_sdp_string(&sdp_answer))
        {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(%participant_id, "invalid SDP answer: {e}");
                return;
            }
        };

        if let Some(pending) = participant.pending_offer.take() {
            if let Err(e) = participant.rtc.sdp_api().accept_answer(pending, answer) {
                tracing::warn!(%participant_id, "failed to accept SDP answer: {e}");
            } else {
                // Collect origins that just became Open (need keyframe request)
                let mut newly_opened: Vec<(ParticipantId, Mid)> = Vec::new();

                for track in &mut participant.tracks_out {
                    if let TrackOutState::Negotiating(mid) = track.state {
                        track.state = TrackOutState::Open(mid);
                        if track.kind == MediaKind::Video {
                            newly_opened.push((track.origin, track.origin_mid));
                        }
                    }
                }

                // Request keyframes from origins so the subscriber gets an IDR
                // immediately. Without this, the first keyframe was likely sent
                // BEFORE the track went Open (and got dropped), so the subscriber
                // waits 2-10s for the next periodic keyframe.
                for (origin_pid, origin_mid) in newly_opened {
                    if let Some(origin) = session.participants.get_mut(&origin_pid) {
                        if let Some(mut writer) = origin.rtc.writer(origin_mid) {
                            match writer.request_keyframe(None, KeyframeRequestKind::Pli) {
                                Ok(()) => {
                                    tracing::info!(
                                        subscriber = %participant_id,
                                        publisher = %origin_pid,
                                        %origin_mid,
                                        "PLI requested after track opened"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        subscriber = %participant_id,
                                        publisher = %origin_pid,
                                        %origin_mid,
                                        "PLI request failed: {e}"
                                    );
                                }
                            }
                        } else {
                            tracing::warn!(
                                %origin_pid, %origin_mid,
                                "no writer for origin (cannot request PLI)"
                            );
                        }
                    }
                }
            }
        }
    }

    fn handle_ice_candidate(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
        candidate: String,
        _sdp_mid: Option<String>,
    ) {
        tracing::info!(%participant_id, %candidate, "received remote ICE candidate");

        let Some(session) = self.sessions.get_mut(&session_id) else {
            tracing::warn!(%session_id, "session not found for ICE candidate");
            return;
        };
        let Some(participant) = session.participants.get_mut(&participant_id) else {
            tracing::warn!(%participant_id, "participant not found for ICE candidate");
            return;
        };

        match Candidate::from_sdp_string(&candidate) {
            Ok(c) => {
                participant.rtc.add_remote_candidate(c);
            }
            Err(e) => {
                tracing::debug!(%participant_id, "ignoring unparseable ICE candidate: {e}");
            }
        }
    }

    fn handle_leave(&mut self, session_id: SessionId, participant_id: ParticipantId) {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return;
        };

        if let Some(mut participant) = session.participants.remove(&participant_id) {
            participant.rtc.disconnect();
            tracing::info!(%session_id, %participant_id, "participant left SFU");
        }

        // Collect Open MIDs before removing TrackOut entries (need them for set_direction)
        let mut mids_to_deactivate: Vec<(ParticipantId, Vec<Mid>)> = Vec::new();
        for (pid, remaining) in &session.participants {
            let mids: Vec<Mid> = remaining
                .tracks_out
                .iter()
                .filter(|t| t.origin == participant_id)
                .filter_map(|t| match t.state {
                    TrackOutState::Open(mid) | TrackOutState::Negotiating(mid) => Some(mid),
                    _ => None,
                })
                .collect();
            if !mids.is_empty() {
                mids_to_deactivate.push((*pid, mids));
            }
        }

        // Remove stale TrackOut entries
        let mut cleaned = 0usize;
        for remaining in session.participants.values_mut() {
            let before = remaining.tracks_out.len();
            remaining.tracks_out.retain(|t| t.origin != participant_id);
            cleaned += before - remaining.tracks_out.len();
        }
        if cleaned > 0 {
            tracing::info!(%session_id, %participant_id, cleaned, "cleaned stale TrackOut entries");
        }

        // Renegotiate: set dead transceivers to Inactive so SDP doesn't accumulate
        for (pid, mids) in mids_to_deactivate {
            let Some(p) = session.participants.get_mut(&pid) else {
                continue;
            };
            // Skip if already waiting for an answer
            if p.pending_offer.is_some() {
                tracing::debug!(%pid, "skipping deactivation renegotiation (pending offer)");
                continue;
            }

            let mut change = p.rtc.sdp_api();
            for &mid in &mids {
                change.set_direction(mid, Direction::Inactive);
            }

            if !change.has_changes() {
                continue;
            }

            match change.apply() {
                Some((offer, pending)) => {
                    p.pending_offer = Some(pending);
                    let offer_str = offer.to_sdp_string();
                    let _ = p.ws_tx.send(ServerMessage::Offer {
                        sdp_offer: offer_str,
                        tracks: None,
                    });
                    tracing::info!(%pid, mids = ?mids, "sent deactivation renegotiation offer");
                }
                None => {
                    tracing::debug!(%pid, "deactivation apply() returned None");
                }
            }
        }

        if session.participants.is_empty() {
            self.sessions.remove(&session_id);
            tracing::info!(%session_id, "SFU session destroyed (empty)");
        }
    }

    fn handle_udp_packet(&mut self, data: &[u8], source: SocketAddr) {
        let Ok(contents) = data.try_into() else {
            return;
        };

        let input = Input::Receive(
            Instant::now(),
            Receive {
                proto: Protocol::Udp,
                source,
                destination: self.candidate_addr,
                contents,
            },
        );

        // Find which Rtc accepts this packet
        for session in self.sessions.values_mut() {
            for participant in session.participants.values_mut() {
                if participant.rtc.accepts(&input) {
                    participant.last_activity_at = Instant::now();
                    if let Err(e) = participant.rtc.handle_input(input) {
                        tracing::warn!(id = %participant.id, "rtc handle_input error: {e}");
                        participant.rtc.disconnect();
                    }
                    return;
                }
            }
        }

        tracing::debug!(%source, bytes = data.len(), "no Rtc accepts UDP packet");
    }

    fn tick(&mut self) {
        let now = Instant::now();

        // Collect zombies: ICE disconnected + no media for 30s.
        // At 500 participants this is a single O(N) scan per tick.
        let mut zombies: Vec<(SessionId, ParticipantId)> = Vec::new();

        for (session_id, session) in &mut self.sessions {
            for participant in session.participants.values_mut() {
                let _ = participant.rtc.handle_input(Input::Timeout(now));

                if participant.ice_disconnected
                    && participant.last_activity_at.elapsed() > Duration::from_secs(30)
                {
                    tracing::warn!(
                        pid = %participant.id,
                        secs_since_activity = participant.last_activity_at.elapsed().as_secs(),
                        "zombie participant (ICE disconnected, no UDP activity)"
                    );
                    zombies.push((*session_id, participant.id));
                }
            }
        }

        // Disconnect zombies (will be cleaned up by WS close or next Leave command)
        for (session_id, pid) in zombies {
            if let Some(session) = self.sessions.get_mut(&session_id) {
                if let Some(p) = session.participants.get_mut(&pid) {
                    p.rtc.disconnect();
                }
            }
        }
    }

    /// Poll all Rtc instances and forward media IMMEDIATELY (interleaved).
    ///
    /// Old architecture (BUG-1): collect all MediaData into a Vec, then forward in a second pass.
    /// The delay between polling and forwarding grew with participant count.
    ///
    /// New architecture: for each participant, poll -> on MediaData, write to every
    /// subscriber and drain their transmit queues RIGHT AWAY. The packet hits UDP
    /// within microseconds of being polled, not after we finish iterating everyone.
    fn poll_all_outputs(&mut self) {
        let session_ids: Vec<SessionId> = self.sessions.keys().copied().collect();

        for session_id in session_ids {
            let pids: Vec<ParticipantId> = match self.sessions.get(&session_id) {
                Some(s) => s.participants.keys().copied().collect(),
                None => continue,
            };

            for &source_pid in &pids {
                loop {
                    // Scope the mutable borrow: extract output, then release self.sessions
                    let output = {
                        let Some(session) = self.sessions.get_mut(&session_id) else {
                            break;
                        };
                        let Some(p) = session.participants.get_mut(&source_pid) else {
                            break;
                        };
                        if !p.rtc.is_alive() {
                            break;
                        }
                        p.rtc.poll_output()
                    };
                    // Borrow on self.sessions released -- we can re-borrow freely below

                    match output {
                        Ok(Output::Transmit(transmit)) => {
                            tracing::debug!(
                                %source_pid,
                                dest = %transmit.destination,
                                bytes = transmit.contents.len(),
                                "UDP SEND"
                            );
                            if let Err(e) = self
                                .udp_socket
                                .try_send_to(&transmit.contents, transmit.destination)
                            {
                                tracing::warn!("UDP send dropped: {e}");
                            }
                        }
                        Ok(Output::Event(event)) => match event {
                            Event::IceConnectionStateChange(state) => {
                                tracing::info!(%source_pid, ?state, "ICE state changed");
                                if let Some(session) = self.sessions.get_mut(&session_id) {
                                    if let Some(p) = session.participants.get_mut(&source_pid) {
                                        p.ice_disconnected = matches!(
                                            state,
                                            IceConnectionState::Disconnected
                                        );
                                    }
                                }
                            }
                            Event::MediaAdded(e) => {
                                tracing::info!(
                                    %source_pid,
                                    mid = %e.mid,
                                    kind = ?e.kind,
                                    "media track added"
                                );
                                // Record on source + queue TrackOut on all others (single pass)
                                if let Some(session) = self.sessions.get_mut(&session_id) {
                                    for (pid, p) in &mut session.participants {
                                        if *pid == source_pid {
                                            p.tracks_in.push(TrackIn {
                                                mid: e.mid,
                                                kind: e.kind,
                                            });
                                        } else {
                                            let already = p.tracks_out.iter().any(|t| {
                                                t.origin == source_pid && t.origin_mid == e.mid
                                            });
                                            if !already {
                                                p.tracks_out.push(TrackOut {
                                                    origin: source_pid,
                                                    origin_mid: e.mid,
                                                    kind: e.kind,
                                                    state: TrackOutState::ToOpen,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            Event::MediaData(data) => {
                                // Count stats by kind
                                if let Some(session) = self.sessions.get_mut(&session_id) {
                                    if let Some(p) = session.participants.get_mut(&source_pid) {
                                        let is_audio = p.tracks_in.iter().any(|t| {
                                            t.mid == data.mid && t.kind == MediaKind::Audio
                                        });
                                        if is_audio {
                                            self.stats_audio_fwd += 1;
                                        } else {
                                            self.stats_video_fwd += 1;
                                        }
                                    }
                                }
                                self.forward_media_now(session_id, source_pid, &data);
                            }
                            Event::KeyframeRequest(req) => {
                                // Route PLI/FIR to the PUBLISHER, not the subscriber.
                                // req.mid is on the subscriber's Rtc. Find which TrackOut
                                // it belongs to and request from the origin.
                                if let Some(session) = self.sessions.get_mut(&session_id) {
                                    let origin =
                                        session.participants.get(&source_pid).and_then(|p| {
                                            p.tracks_out.iter().find_map(|t| {
                                                if t.open_mid() == Some(req.mid) {
                                                    Some((t.origin, t.origin_mid))
                                                } else {
                                                    None
                                                }
                                            })
                                        });
                                    if let Some((origin_pid, origin_mid)) = origin
                                        && let Some(origin_p) =
                                            session.participants.get_mut(&origin_pid)
                                        && let Some(mut w) = origin_p.rtc.writer(origin_mid)
                                    {
                                        let _ = w.request_keyframe(req.rid, req.kind);
                                    }
                                }
                            }
                            _ => {}
                        },
                        Ok(Output::Timeout(_)) => break,
                        Err(e) => {
                            tracing::warn!(%source_pid, "poll_output error: {e}");
                            if let Some(session) = self.sessions.get_mut(&session_id)
                                && let Some(p) = session.participants.get_mut(&source_pid)
                            {
                                p.rtc.disconnect();
                            }
                            break;
                        }
                    }
                }
            }
        }

        // Negotiate tracks in a separate pass -- batch all ToOpen into one SDP offer per participant
        self.negotiate_pending_tracks();
    }

    /// Forward one MediaData packet to every subscriber immediately.
    /// write() + drain_transmits per target so the RTP packet hits UDP
    /// within the same poll iteration that produced it.
    fn forward_media_now(
        &mut self,
        session_id: SessionId,
        source_pid: ParticipantId,
        data: &MediaData,
    ) {
        // Immutable borrow: collect (target_pid, outgoing_mid) pairs
        let targets: Vec<(ParticipantId, Mid)> = {
            let Some(session) = self.sessions.get(&session_id) else {
                return;
            };
            session
                .participants
                .iter()
                .filter(|(pid, _)| **pid != source_pid)
                .filter_map(|(pid, p)| {
                    p.tracks_out
                        .iter()
                        .find(|t| t.origin == source_pid && t.origin_mid == data.mid)
                        .and_then(|t| t.open_mid())
                        .map(|mid| (*pid, mid))
                })
                .collect()
        };
        // Immutable borrow released

        for (target_pid, mid) in targets {
            let Some(session) = self.sessions.get_mut(&session_id) else {
                return;
            };
            let Some(target) = session.participants.get_mut(&target_pid) else {
                continue;
            };

            let Some(writer) = target.rtc.writer(mid) else {
                continue;
            };
            let Some(pt) = writer.match_params(data.params) else {
                continue;
            };

            if let Err(e) = writer.write(pt, data.network_time, data.time, data.data.clone()) {
                tracing::trace!(pid = %target_pid, "media write skip: {e}");
                continue;
            }

            // Drain transmits right after write -- the RTP packet hits the wire NOW
            loop {
                match target.rtc.poll_output() {
                    Ok(Output::Transmit(t)) => {
                        if let Err(e) = self.udp_socket.try_send_to(&t.contents, t.destination) {
                            tracing::warn!("UDP drain dropped: {e}");
                        }
                    }
                    Ok(Output::Timeout(_)) => break,
                    Ok(Output::Event(_)) => {} // events handled in main poll loop
                    Err(_) => break,
                }
            }
        }
    }

    /// Create SDP offers for all participants that have ToOpen outgoing tracks.
    /// Batches multiple ToOpen tracks into a single offer per participant.
    fn negotiate_pending_tracks(&mut self) {
        let mut to_negotiate: Vec<(SessionId, ParticipantId)> = Vec::new();
        for (session_id, session) in &self.sessions {
            for (pid, participant) in &session.participants {
                let has_to_open = participant
                    .tracks_out
                    .iter()
                    .any(|t| matches!(t.state, TrackOutState::ToOpen));
                if has_to_open {
                    to_negotiate.push((*session_id, *pid));
                }
            }
        }

        for (session_id, pid) in to_negotiate {
            let Some(session) = self.sessions.get_mut(&session_id) else {
                continue;
            };

            // Pre-collect track mapping (before mutable borrow of participant)
            let track_mappings: Vec<crate::signaling::messages::TrackMapping> = session
                .participants
                .get(&pid)
                .map(|p| {
                    p.tracks_out
                        .iter()
                        .filter_map(|t| {
                            let origin_p = session.participants.get(&t.origin)?;
                            Some(crate::signaling::messages::TrackMapping {
                                stream_id: t.origin.to_string(),
                                participant_id: t.origin,
                                user_id: origin_p.user_id.clone(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            let Some(participant) = session.participants.get_mut(&pid) else {
                continue;
            };

            if participant.pending_offer.is_some() {
                continue;
            }

            let mut change = participant.rtc.sdp_api();
            let mut has_changes = false;

            for track in &mut participant.tracks_out {
                if let TrackOutState::ToOpen = track.state {
                    let mid = change.add_media(
                        track.kind,
                        Direction::SendOnly,
                        Some(track.origin.to_string()),
                        None,
                        None,
                    );
                    track.state = TrackOutState::Negotiating(mid);
                    has_changes = true;
                }
            }

            if !has_changes {
                continue;
            }

            match change.apply() {
                Some((offer, pending)) => {
                    participant.pending_offer = Some(pending);
                    let offer_str = offer.to_sdp_string();

                    let _ = participant.ws_tx.send(ServerMessage::Offer {
                        sdp_offer: offer_str,
                        tracks: if track_mappings.is_empty() {
                            None
                        } else {
                            Some(track_mappings.clone())
                        },
                    });
                    tracing::info!(%pid, "sent renegotiation offer");
                }
                None => {
                    tracing::warn!(%pid, "sdp_api().apply() returned None");
                }
            }
        }
    }
}
