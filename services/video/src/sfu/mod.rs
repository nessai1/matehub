pub mod session;
pub mod udp;

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam::channel::{Receiver, TryRecvError};
use str0m::change::SdpOffer;
use str0m::media::{Direction, MediaData, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::signaling::ServerMessage;

pub use session::{SfuParticipant, SfuSession, Source, TrackIn, TrackOut, TrackOutState};

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
        // Still tokio mpsc — the receiving side is the per-WebSocket send task
        // (async). We only call `.send()` which is sync on tokio mpsc, so the
        // media thread can talk to the tokio runtime without blocking.
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
    /// Source hint for the next MediaAdded of the given kind.
    /// Arrives before the matching client-initiated Offer.
    PublishTrack {
        session_id: SessionId,
        participant_id: ParticipantId,
        source: Source,
        kind: MediaKind,
    },
    /// Client-initiated SDP offer (renegotiation after addTrack/removeTrack).
    ClientOffer {
        session_id: SessionId,
        participant_id: ParticipantId,
        sdp_offer: String,
    },
}

/// The SFU engine. Runs on its own OS thread (not a tokio task) so media
/// forwarding latency doesn't compete with signaling / HTTP work on the
/// shared tokio runtime.
pub struct SfuEngine {
    sessions: HashMap<SessionId, SfuSession>,
    /// Blocking std socket. `set_read_timeout` gives us the tick cadence;
    /// send_to is blocking but kernel UDP send buffer (tuned to 2MB in main)
    /// makes the blocking window negligible in practice.
    udp_socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    /// Host candidate addresses advertised to every peer.
    /// Multiple IPs (wifi, ethernet, vpn) so ICE can pick whichever
    /// actually routes back to the server — avoids peer-reflexive
    /// fallback when the server is bound on 0.0.0.0 across interfaces.
    candidate_addrs: Vec<SocketAddr>,
    /// crossbeam channel: tokio WS handlers (multi-producer) → media thread.
    /// tokio::sync::mpsc is async-only, can't be polled from a blocking thread.
    cmd_rx: Receiver<SfuCommand>,
    /// Remote-addr → participant cache for O(1) UDP demux.
    /// Populated after the first successful accept() from a given source addr.
    /// Invalidated on leave / session destroy / stale mapping detection.
    addr_to_participant: HashMap<SocketAddr, (SessionId, ParticipantId)>,
    /// True when at least one participant has a TrackOut in ToOpen state.
    /// Gates the O(N×K) sweep in `negotiate_pending_tracks()` — 50 times/sec
    /// on audio alone is a waste when there's nothing to negotiate.
    has_pending_negotiation: bool,
    /// Media forwarding stats (logged periodically)
    stats_audio_fwd: u64,
    stats_video_fwd: u64,
    stats_last_log: Instant,
}

/// Build a per-(source, kind) stream id for a forwarded track.
///
/// Chrome treats tracks sharing an `a=msid:` stream as one MediaStream and
/// enables A/V sync on playout. If camera+screen shared an msid, or if
/// audio+video shared one, Chrome would hold the smaller/later track's
/// playout hostage to the other's jitter buffer. Splitting by (source, kind)
/// gives four independent streams per publisher:
///
///   `<uuid>-cam-audio`, `<uuid>-cam-video`,
///   `<uuid>-screen-audio`, `<uuid>-screen-video`.
///
/// Separator is `-` because RFC 7941 msid-id allows only `[A-Za-z0-9-._~]`;
/// str0m strips anything else, which would desync TrackMapping (sent as-is
/// over signaling) from the msid the browser sees on `ontrack`.
fn stream_id_for(origin: ParticipantId, source: Source, kind: MediaKind) -> String {
    let source_tag = source.as_msid_tag();
    let kind_tag = match kind {
        MediaKind::Audio => "audio",
        MediaKind::Video => "video",
    };
    format!("{origin}-{source_tag}-{kind_tag}")
}

/// What work a just-handled event produced. Drives targeted polling instead
/// of sweeping every Rtc after every event.
#[derive(Debug, Clone, Copy)]
enum PollTarget {
    /// A single participant has input waiting — poll just that one.
    One(SessionId, ParticipantId),
    /// Multiple participants in a session may have work (e.g. deactivation
    /// renegotiation after Leave) — poll everyone in the session.
    Session(SessionId),
}

impl SfuEngine {
    pub fn new(
        udp_socket: Arc<UdpSocket>,
        public_ips: Vec<std::net::IpAddr>,
        cmd_rx: Receiver<SfuCommand>,
    ) -> Self {
        let local_addr = udp_socket.local_addr().expect("UDP local addr");
        let candidate_addrs: Vec<SocketAddr> = public_ips
            .into_iter()
            .map(|ip| SocketAddr::new(ip, local_addr.port()))
            .collect();
        assert!(
            !candidate_addrs.is_empty(),
            "SfuEngine needs at least one public IP for host candidates"
        );
        Self {
            sessions: HashMap::new(),
            udp_socket,
            local_addr,
            candidate_addrs,
            cmd_rx,
            addr_to_participant: HashMap::new(),
            has_pending_negotiation: false,
            stats_audio_fwd: 0,
            stats_video_fwd: 0,
            stats_last_log: Instant::now(),
        }
    }

    /// First candidate address — used as `destination` when constructing
    /// `Input::Receive` (str0m only cares about port + protocol, not which
    /// specific bound IP the datagram landed on).
    fn primary_candidate_addr(&self) -> SocketAddr {
        self.candidate_addrs[0]
    }

    /// Main SFU event loop. Call this from a dedicated `std::thread` — NOT
    /// a tokio task.
    ///
    /// The event loop is a single blocking thread:
    /// 1. `recv_from` with `SO_RCVTIMEO` = 20ms — returns either a datagram
    ///    or `TimedOut`/`WouldBlock`.
    /// 2. Drain all pending commands from the crossbeam channel (non-blocking).
    /// 3. If the tick deadline is due, fire `tick() + poll_all()`.
    /// 4. Run pending negotiations.
    ///
    /// Why this over `tokio::select!`:
    /// - Media forwarding runs on its own OS thread so signaling / HTTP / WS
    ///   activity on the tokio runtime can't preempt a packet forward.
    /// - `poll_output` → `send_to` is a synchronous sequence; no `.await`
    ///   yield points between "read RTP" and "write RTP", which removes a
    ///   whole class of latency jitter.
    pub fn run_blocking(mut self) {
        let mut buf = vec![0u8; 2000];
        const TICK: Duration = Duration::from_millis(20);

        // SO_RCVTIMEO: recv_from returns TimedOut after 20ms of silence. That
        // gives us a deterministic tick cadence without an extra timer thread.
        // Using 20ms once (not per-iteration) so we don't hammer setsockopt.
        if let Err(e) = self.udp_socket.set_read_timeout(Some(TICK)) {
            tracing::error!("failed to set UDP read timeout: {e}");
            return;
        }

        let mut next_tick = Instant::now() + TICK;
        tracing::info!(local_addr = %self.local_addr, "SFU engine started (dedicated thread)");

        loop {
            // --- 1. UDP receive (blocking up to TICK) -----------------------
            match self.udp_socket.recv_from(&mut buf) {
                Ok((n, source)) => {
                    if let Some((sid, pid)) = self.handle_udp_packet(&buf[..n], source) {
                        self.poll_participant(sid, pid);
                    }
                }
                Err(e)
                    if e.kind() == ErrorKind::WouldBlock
                        || e.kind() == ErrorKind::TimedOut =>
                {
                    // Tick fires below.
                }
                Err(e) => {
                    tracing::error!("UDP recv error: {e}");
                }
            }

            // --- 2. Drain pending commands ---------------------------------
            let mut targets: Vec<PollTarget> = Vec::new();
            loop {
                match self.cmd_rx.try_recv() {
                    Ok(cmd) => {
                        if let Some(t) = self.handle_command(cmd) {
                            targets.push(t);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        tracing::info!("SFU command channel closed, shutting down");
                        return;
                    }
                }
            }
            for target in targets {
                match target {
                    PollTarget::One(sid, pid) => self.poll_participant(sid, pid),
                    PollTarget::Session(sid) => self.poll_session(sid),
                }
            }

            // --- 3. Tick ----------------------------------------------------
            let now = Instant::now();
            if now >= next_tick {
                self.tick();
                // tick() pushed Input::Timeout into every Rtc; sweep to emit
                // any pending Transmit/Event.
                self.poll_all();

                // Stats log every 5s.
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

                // Skip missed ticks instead of bursting — if we lagged behind
                // (e.g. a big keyframe cascade), snap forward rather than
                // firing tick() several times in a row.
                next_tick = now + TICK;
            }

            // --- 4. Negotiation (flag-gated) --------------------------------
            if self.has_pending_negotiation {
                self.negotiate_pending_tracks();
            }
        }
    }

    fn handle_command(&mut self, cmd: SfuCommand) -> Option<PollTarget> {
        match cmd {
            SfuCommand::Join {
                session_id,
                participant_id,
                user_id,
                sdp_offer,
                reply_tx,
            } => self.handle_join(session_id, participant_id, user_id, sdp_offer, reply_tx),
            SfuCommand::Answer {
                session_id,
                participant_id,
                sdp_answer,
            } => self.handle_answer(session_id, participant_id, sdp_answer),
            SfuCommand::IceCandidate {
                session_id,
                participant_id,
                candidate,
                sdp_mid,
            } => self.handle_ice_candidate(session_id, participant_id, candidate, sdp_mid),
            SfuCommand::Leave {
                session_id,
                participant_id,
            } => self.handle_leave(session_id, participant_id),
            SfuCommand::PublishTrack {
                session_id,
                participant_id,
                source,
                kind,
            } => self.handle_publish_track(session_id, participant_id, source, kind),
            SfuCommand::ClientOffer {
                session_id,
                participant_id,
                sdp_offer,
            } => self.handle_client_offer(session_id, participant_id, sdp_offer),
        }
    }

    /// Record a source hint for the next MediaAdded of the given kind from
    /// this publisher. Arrives right before a client-initiated Offer.
    fn handle_publish_track(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
        source: Source,
        kind: MediaKind,
    ) -> Option<PollTarget> {
        let session = self.sessions.get_mut(&session_id)?;
        let participant = session.participants.get_mut(&participant_id)?;
        participant
            .pending_source_hints
            .entry(kind)
            .or_default()
            .push_back(source);
        tracing::info!(%participant_id, ?source, ?kind, "queued source hint");
        // No polling needed — hint is consumed when the SDP offer follows.
        None
    }

    /// Accept a client-initiated SDP offer (renegotiation after the client
    /// added/removed tracks — e.g. `pc.addTrack(screenVideoTrack)`).
    fn handle_client_offer(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
        sdp_offer: String,
    ) -> Option<PollTarget> {
        let offer: SdpOffer = match serde_json::from_str(&sdp_offer) {
            Ok(o) => o,
            Err(_) => match SdpOffer::from_sdp_string(&sdp_offer) {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!(%participant_id, "invalid client SDP offer: {e}");
                    return None;
                }
            },
        };

        let session = self.sessions.get_mut(&session_id)?;
        let participant = session.participants.get_mut(&participant_id)?;

        // A pending server-side offer would make this impossible to reconcile —
        // the client's offer is relative to the last stable state. In practice
        // SDK waits for our last Answer before issuing its own Offer; log and
        // drop if that invariant gets broken.
        if participant.pending_offer.is_some() {
            tracing::warn!(
                %participant_id,
                "client offer arrived while server offer still pending — dropping"
            );
            return None;
        }

        let answer = match participant.rtc.sdp_api().accept_offer(offer) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(%participant_id, "client offer rejected: {e}");
                return None;
            }
        };

        let answer_str = answer.to_sdp_string();
        let _ = participant.ws_tx.send(ServerMessage::Answer {
            sdp_answer: answer_str,
            participant_id,
        });

        tracing::info!(%participant_id, "client offer accepted, answer sent");
        Some(PollTarget::One(session_id, participant_id))
    }

    fn handle_join(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
        user_id: String,
        sdp_offer: String,
        reply_tx: mpsc::UnboundedSender<ServerMessage>,
    ) -> Option<PollTarget> {
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
                    return None;
                }
            },
        };

        // Create Rtc instance
        // Note: full ICE (not ICE-lite). ICE-lite causes immediate Disconnected
        // because str0m expects STUN before poll_output runs the first timeout.
        let mut rtc = Rtc::new();

        // Add one host candidate per local interface we know about.
        // ICE on the browser then has multiple real paths to try instead of
        // falling back to peer-reflexive when the advertised IP isn't the
        // one the kernel actually sends from.
        for addr in &self.candidate_addrs {
            let candidate = Candidate::host(*addr, "udp").expect("host candidate");
            rtc.add_local_candidate(candidate);
        }

        // Accept the offer and generate answer
        let answer = match rtc.sdp_api().accept_offer(offer) {
            Ok(answer) => answer,
            Err(e) => {
                tracing::warn!(%participant_id, "failed to accept SDP offer: {e}");
                let _ = reply_tx.send(ServerMessage::Error {
                    message: format!("SDP negotiation failed: {e}"),
                });
                return None;
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
        let existing_tracks: Vec<(ParticipantId, Mid, MediaKind, Source)> = session
            .participants
            .values()
            .flat_map(|p| {
                p.tracks_in
                    .iter()
                    .map(|t| (p.id, t.mid, t.kind, t.source))
            })
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
            pending_source_hints: HashMap::new(),
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
            for (origin_pid, mid, kind, source) in &existing_tracks {
                new_participant.tracks_out.push(TrackOut {
                    origin: *origin_pid,
                    origin_mid: *mid,
                    kind: *kind,
                    source: *source,
                    state: TrackOutState::ToOpen,
                });
            }
            tracing::info!(
                %participant_id,
                existing = existing_tracks.len(),
                "queued existing tracks for new participant"
            );
            self.has_pending_negotiation = true;
        }

        tracing::info!(%session_id, %participant_id, "participant joined SFU");
        Some(PollTarget::One(session_id, participant_id))
    }

    fn handle_answer(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
        sdp_answer: String,
    ) -> Option<PollTarget> {
        let session = self.sessions.get_mut(&session_id)?;
        let participant = session.participants.get_mut(&participant_id)?;

        let answer = match serde_json::from_str(&sdp_answer)
            .or_else(|_| str0m::change::SdpAnswer::from_sdp_string(&sdp_answer))
        {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(%participant_id, "invalid SDP answer: {e}");
                return None;
            }
        };

        if let Some(pending) = participant.pending_offer.take() {
            if let Err(e) = participant.rtc.sdp_api().accept_answer(pending, answer) {
                tracing::warn!(%participant_id, "failed to accept SDP answer: {e}");
            } else {
                // Collect origins that just became Open (need keyframe request)
                let mut newly_opened: Vec<(ParticipantId, Mid)> = Vec::new();
                // Collect forwarding entries to register after participant
                // borrow is released.
                let mut to_register: Vec<((ParticipantId, Mid), (ParticipantId, Mid))> =
                    Vec::new();

                for track in &mut participant.tracks_out {
                    if let TrackOutState::Negotiating(mid) = track.state {
                        track.state = TrackOutState::Open(mid);
                        if track.kind == MediaKind::Video {
                            newly_opened.push((track.origin, track.origin_mid));
                        }
                        to_register
                            .push(((track.origin, track.origin_mid), (participant_id, mid)));
                    }
                }

                // Register fan-out entries (publishers → new subscriber).
                for (key, pair) in to_register {
                    session
                        .forwarding_map
                        .entry(key)
                        .or_default()
                        .push(pair);
                }

                // Request keyframes from origins so the subscriber gets an IDR
                // immediately. Without this, the first keyframe was likely sent
                // BEFORE the track went Open (and got dropped), so the subscriber
                // waits 2-10s for the next periodic keyframe.
                //
                // Throttled: when many subscribers ack a screen-share offer in
                // the same RTT, only one PLI per (publisher, mid) actually goes
                // out — the rest find a fresh entry in last_keyframe_at and
                // skip. The single keyframe that does get generated is
                // received by everyone newly opened.
                for (origin_pid, origin_mid) in newly_opened {
                    let sent = session.request_keyframe_throttled(origin_pid, origin_mid);
                    if sent {
                        tracing::info!(
                            subscriber = %participant_id,
                            publisher = %origin_pid,
                            %origin_mid,
                            "PLI requested after track opened"
                        );
                    } else {
                        tracing::debug!(
                            subscriber = %participant_id,
                            publisher = %origin_pid,
                            %origin_mid,
                            "PLI suppressed (throttle / writer unavailable)"
                        );
                    }
                }
            }
        }
        // Answer may have opened new tracks and triggered PLI against origins —
        // sweep the whole session so those keyframe requests leave the wire.
        Some(PollTarget::Session(session_id))
    }

    fn handle_ice_candidate(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
        candidate: String,
        _sdp_mid: Option<String>,
    ) -> Option<PollTarget> {
        tracing::debug!(%participant_id, %candidate, "received remote ICE candidate");

        let session = match self.sessions.get_mut(&session_id) {
            Some(s) => s,
            None => {
                tracing::warn!(%session_id, "session not found for ICE candidate");
                return None;
            }
        };
        let participant = match session.participants.get_mut(&participant_id) {
            Some(p) => p,
            None => {
                tracing::warn!(%participant_id, "participant not found for ICE candidate");
                return None;
            }
        };

        match Candidate::from_sdp_string(&candidate) {
            Ok(c) => {
                participant.rtc.add_remote_candidate(c);
                Some(PollTarget::One(session_id, participant_id))
            }
            Err(e) => {
                tracing::debug!(%participant_id, "ignoring unparseable ICE candidate: {e}");
                None
            }
        }
    }

    fn handle_leave(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
    ) -> Option<PollTarget> {
        // Drop addr cache entries for this participant first — cheap and keeps
        // the mapping from pointing at a dead Rtc.
        self.addr_to_participant
            .retain(|_, &mut (sid, pid)| !(sid == session_id && pid == participant_id));

        let session = self.sessions.get_mut(&session_id)?;

        if let Some(mut participant) = session.participants.remove(&participant_id) {
            participant.rtc.disconnect();
            tracing::info!(%session_id, %participant_id, "participant left SFU");
        }

        // Drop fan-out entries referencing the gone participant (as publisher
        // or subscriber). Keeps forward_media_now's O(1) lookup truthful.
        session.drop_from_forwarding(participant_id);

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
            // Defensive: sweep any stragglers still pointing at this session.
            self.addr_to_participant
                .retain(|_, &mut (sid, _)| sid != session_id);
            tracing::info!(%session_id, "SFU session destroyed (empty)");
            return None;
        }
        // Deactivation offers were queued for remaining participants — sweep
        // the session so those offers' transmits hit the wire.
        Some(PollTarget::Session(session_id))
    }

    fn handle_udp_packet(
        &mut self,
        data: &[u8],
        source: SocketAddr,
    ) -> Option<(SessionId, ParticipantId)> {
        // Fast path: O(1) lookup by remote addr. Populated after the first
        // successful accept() from this source.
        if let Some((session_id, participant_id)) =
            self.addr_to_participant.get(&source).copied()
        {
            let Ok(contents) = data.try_into() else {
                return None;
            };
            let input = Input::Receive(
                Instant::now(),
                Receive {
                    proto: Protocol::Udp,
                    source,
                    destination: self.primary_candidate_addr(),
                    contents,
                },
            );

            let handled = if let Some(session) = self.sessions.get_mut(&session_id) {
                if let Some(p) = session.participants.get_mut(&participant_id) {
                    if p.rtc.accepts(&input) {
                        p.last_activity_at = Instant::now();
                        let pid = p.id;
                        if let Err(e) = p.rtc.handle_input(input) {
                            tracing::warn!(id = %pid, "rtc handle_input error: {e}");
                            p.rtc.disconnect();
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

            if handled {
                return Some((session_id, participant_id));
            }
            // Stale mapping: participant/session gone, or ICE restart rebound
            // the Rtc to a new source. Drop the cache entry and fall through.
            self.addr_to_participant.remove(&source);
        }

        // Slow path: linear scan. Used only for unknown source addrs — first
        // packet from a new peer, STUN bindings, or ICE restarts.
        let now = Instant::now();
        // Pull the destination addr out before the &mut borrow of self.sessions.
        let destination = self.primary_candidate_addr();

        let mut matched: Option<(SessionId, ParticipantId)> = None;
        for (session_id, session) in self.sessions.iter_mut() {
            for participant in session.participants.values_mut() {
                // Input is !Copy (handle_input consumes), and Receive::contents
                // is !Copy too — so we build a fresh one each time we need it.
                // data.try_into() is a cheap wrapper over the slice.
                let Ok(probe_contents) = data.try_into() else {
                    return None;
                };
                let probe = Input::Receive(
                    now,
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination,
                        contents: probe_contents,
                    },
                );
                if !participant.rtc.accepts(&probe) {
                    continue;
                }

                let Ok(input_contents) = data.try_into() else {
                    return None;
                };
                let input = Input::Receive(
                    now,
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination,
                        contents: input_contents,
                    },
                );

                participant.last_activity_at = now;
                let pid = participant.id;
                if let Err(e) = participant.rtc.handle_input(input) {
                    tracing::warn!(id = %pid, "rtc handle_input error: {e}");
                    participant.rtc.disconnect();
                    return None;
                }
                matched = Some((*session_id, pid));
                break;
            }
            if matched.is_some() {
                break;
            }
        }

        match matched {
            Some((sid, pid)) => {
                self.addr_to_participant.insert(source, (sid, pid));
                Some((sid, pid))
            }
            None => {
                tracing::debug!(%source, bytes = data.len(), "no Rtc accepts UDP packet");
                None
            }
        }
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

    /// Sweep every Rtc in every session. Used only after the 20ms timer tick;
    /// targeted polls handle the high-frequency UDP/command paths.
    fn poll_all(&mut self) {
        let session_ids: Vec<SessionId> = self.sessions.keys().copied().collect();
        for session_id in session_ids {
            self.poll_session(session_id);
        }
    }

    /// Poll every participant of a single session. Used after Leave (for
    /// deactivation offers) and similar session-wide changes.
    fn poll_session(&mut self, session_id: SessionId) {
        let pids: Vec<ParticipantId> = match self.sessions.get(&session_id) {
            Some(s) => s.participants.keys().copied().collect(),
            None => return,
        };
        for pid in pids {
            self.poll_participant(session_id, pid);
        }
    }

    /// Drain outputs for one participant, forwarding media inline.
    ///
    /// The forwarding architecture (BUG-1 fix): on MediaData we immediately
    /// call write() + drain_transmits on every subscriber — the RTP packet
    /// hits the wire within microseconds of being polled, not after we finish
    /// iterating everyone.
    fn poll_participant(&mut self, session_id: SessionId, source_pid: ParticipantId) {
        loop {
            // Scope the mutable borrow: extract output, then release self.sessions
            let output = {
                let Some(session) = self.sessions.get_mut(&session_id) else {
                    return;
                };
                let Some(p) = session.participants.get_mut(&source_pid) else {
                    return;
                };
                if !p.rtc.is_alive() {
                    return;
                }
                p.rtc.poll_output()
            };
            // Borrow on self.sessions released -- we can re-borrow freely below

            match output {
                Ok(Output::Transmit(transmit)) => {
                    if let Err(e) = self
                        .udp_socket
                        .send_to(&transmit.contents, transmit.destination)
                    {
                        tracing::warn!("UDP send dropped: {e}");
                    }
                }
                Ok(Output::Event(event)) => match event {
                    Event::IceConnectionStateChange(state) => {
                        tracing::info!(%source_pid, ?state, "ICE state changed");
                        if let Some(session) = self.sessions.get_mut(&session_id) {
                            if let Some(p) = session.participants.get_mut(&source_pid) {
                                p.ice_disconnected =
                                    matches!(state, IceConnectionState::Disconnected);
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
                        // Record on source + queue TrackOut on all others (single pass).
                        //
                        // Source resolution: pop a hint from the publisher's
                        // pending queue for this kind (set by `publish_track`
                        // signaling before the client-initiated Offer).
                        // No hint → Camera (the Join-flow default — cam/mic
                        // sent implicitly in the first offer).
                        if let Some(session) = self.sessions.get_mut(&session_id) {
                            let mut queued_for_others = false;
                            let source = session
                                .participants
                                .get_mut(&source_pid)
                                .and_then(|p| {
                                    p.pending_source_hints
                                        .get_mut(&e.kind)
                                        .and_then(|q| q.pop_front())
                                })
                                .unwrap_or(Source::Camera);
                            for (pid, p) in &mut session.participants {
                                if *pid == source_pid {
                                    p.tracks_in.push(TrackIn {
                                        mid: e.mid,
                                        kind: e.kind,
                                        source,
                                        seen_high_layer: false,
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
                                            source,
                                            state: TrackOutState::ToOpen,
                                        });
                                        queued_for_others = true;
                                    }
                                }
                            }
                            if queued_for_others {
                                self.has_pending_negotiation = true;
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
                        // it belongs to and request from the origin — through
                        // the throttle so a 30-viewer fan-in coalesces into
                        // one keyframe instead of N.
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
                            if let Some((origin_pid, origin_mid)) = origin {
                                session.request_keyframe_throttled(origin_pid, origin_mid);
                            }
                        }
                    }
                    _ => {}
                },
                Ok(Output::Timeout(_)) => return,
                Err(e) => {
                    tracing::warn!(%source_pid, "poll_output error: {e}");
                    if let Some(session) = self.sessions.get_mut(&session_id)
                        && let Some(p) = session.participants.get_mut(&source_pid)
                    {
                        p.rtc.disconnect();
                    }
                    return;
                }
            }
        }
    }

    /// Forward one MediaData packet to every subscriber immediately.
    /// write() + drain_transmits per target so the RTP packet hits UDP
    /// within the same poll iteration that produced it.
    ///
    /// Hot path: the subscriber list is looked up in O(1) from
    /// `session.forwarding_map`. To iterate without holding a borrow on the
    /// HashMap while we mutate individual participants, we `mem::take` the
    /// Vec out of the map, iterate, and put it back. An empty Vec never
    /// allocates, so this is zero-alloc when a (pid,mid) already exists
    /// in the map.
    fn forward_media_now(
        &mut self,
        session_id: SessionId,
        source_pid: ParticipantId,
        data: &MediaData,
    ) {
        // Simulcast filter: if the publisher is sending layered video
        // (screen share, mostly), the steady state forwards the `h` layer.
        // Adaptive per-subscriber layer selection lives in Phase 5.
        //
        // Start-up fallback: until the publisher's `h` encoder has produced
        // its first packet (BWE ramp-up takes ~2-5s on a fresh PC),
        // forward `l` so subscribers see something instead of a black
        // screen. Once we see the first `h` packet for this (pid, mid)
        // we flip TrackIn.seen_high_layer and never forward `l` again.
        //
        // `data.rid == None` means no simulcast on this track — always forward.
        if let Some(rid) = data.rid.as_ref() {
            let rid_str: &str = &**rid;
            if rid_str == "h" {
                if let Some(session) = self.sessions.get_mut(&session_id)
                    && let Some(p) = session.participants.get_mut(&source_pid)
                    && let Some(track) =
                        p.tracks_in.iter_mut().find(|t| t.mid == data.mid)
                    && !track.seen_high_layer
                {
                    track.seen_high_layer = true;
                    tracing::info!(
                        publisher = %source_pid,
                        mid = %data.mid,
                        "high simulcast layer live — fallback retired"
                    );
                }
            } else {
                let seen_high = self
                    .sessions
                    .get(&session_id)
                    .and_then(|s| s.participants.get(&source_pid))
                    .and_then(|p| p.tracks_in.iter().find(|t| t.mid == data.mid))
                    .map(|t| t.seen_high_layer)
                    .unwrap_or(false);
                if seen_high {
                    return;
                }
            }
        }

        let key = (source_pid, data.mid);

        // Extract subscriber list without cloning — we put it back below.
        let (mut targets, existed) = {
            let Some(session) = self.sessions.get_mut(&session_id) else {
                return;
            };
            match session.forwarding_map.get_mut(&key) {
                Some(entry) => (std::mem::take(entry), true),
                None => (Vec::new(), false),
            }
        };

        if targets.is_empty() {
            if existed {
                // Was there but empty — drop the stale entry.
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.forwarding_map.remove(&key);
                }
            }
            return;
        }

        for &(target_pid, target_mid) in &targets {
            let Some(session) = self.sessions.get_mut(&session_id) else {
                return;
            };
            let Some(target) = session.participants.get_mut(&target_pid) else {
                continue;
            };

            let Some(writer) = target.rtc.writer(target_mid) else {
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
                        if let Err(e) = self.udp_socket.send_to(&t.contents, t.destination) {
                            tracing::warn!("UDP drain dropped: {e}");
                        }
                    }
                    Ok(Output::Timeout(_)) => break,
                    Ok(Output::Event(_)) => {} // events handled in main poll loop
                    Err(_) => break,
                }
            }
        }

        // Put the Vec back — same allocation, same capacity, no re-scan on
        // the next packet for this publisher/mid.
        if let Some(session) = self.sessions.get_mut(&session_id) {
            match session.forwarding_map.get_mut(&key) {
                Some(entry) => {
                    *entry = std::mem::take(&mut targets);
                }
                None => {
                    // Entry vanished (e.g. publisher left mid-forward). Drop Vec.
                }
            }
        }
    }

    /// Create SDP offers for all participants that have ToOpen outgoing tracks.
    /// Batches multiple ToOpen tracks into a single offer per participant.
    ///
    /// Gated by `self.has_pending_negotiation` — don't call this every
    /// audio packet when there's nothing to negotiate.
    fn negotiate_pending_tracks(&mut self) {
        // Clear the flag up front. Any new ToOpen queued during this pass
        // (e.g. from an SDP apply that triggers another MediaAdded) will
        // set it again.
        self.has_pending_negotiation = false;

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

        if to_negotiate.is_empty() {
            return;
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
                                stream_id: stream_id_for(t.origin, t.source, t.kind),
                                participant_id: t.origin,
                                user_id: origin_p.user_id.clone(),
                                source: match t.source {
                                    Source::Camera => "camera",
                                    Source::Screen => "screen",
                                },
                                kind: match t.kind {
                                    MediaKind::Audio => "audio",
                                    MediaKind::Video => "video",
                                },
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            let Some(participant) = session.participants.get_mut(&pid) else {
                continue;
            };

            if participant.pending_offer.is_some() {
                // Can't send a new offer while we're still waiting for an
                // answer. Re-raise the flag so the next pass retries after
                // handle_answer clears pending_offer.
                self.has_pending_negotiation = true;
                continue;
            }

            let mut change = participant.rtc.sdp_api();
            let mut has_changes = false;

            for track in &mut participant.tracks_out {
                if let TrackOutState::ToOpen = track.state {
                    // Separate msid per (publisher, source, kind). Four
                    // streams per publisher: cam-audio, cam-video,
                    // screen-audio, screen-video. Keeps Chrome from A/V-
                    // syncing mismatched pairs (see stream_id_for docs).
                    let mid = change.add_media(
                        track.kind,
                        Direction::SendOnly,
                        Some(stream_id_for(track.origin, track.source, track.kind)),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7941 msid-id permits [A-Za-z0-9-._~]. Anything outside gets stripped
    /// by str0m, which would desync TrackMapping (sent via signaling as-is)
    /// from the msid the browser sees on `ontrack`.
    fn is_msid_safe(c: char) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')
    }

    const ALL_SOURCES: [Source; 2] = [Source::Camera, Source::Screen];
    const ALL_KINDS: [MediaKind; 2] = [MediaKind::Audio, MediaKind::Video];

    #[test]
    fn stream_id_for_all_four_combinations_are_distinct() {
        let pid = Uuid::new_v4();
        let ids: Vec<String> = ALL_SOURCES
            .iter()
            .flat_map(|&s| ALL_KINDS.iter().map(move |&k| stream_id_for(pid, s, k)))
            .collect();
        // 2 sources × 2 kinds = 4 distinct stream ids — each one is a
        // separate MediaStream on the browser side.
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), 4, "expected 4 unique msid, got {ids:?}");
    }

    #[test]
    fn stream_id_for_tags_embedded_in_output() {
        let pid = Uuid::new_v4();
        assert!(stream_id_for(pid, Source::Camera, MediaKind::Audio).contains("-cam-audio"));
        assert!(stream_id_for(pid, Source::Camera, MediaKind::Video).contains("-cam-video"));
        assert!(stream_id_for(pid, Source::Screen, MediaKind::Audio).contains("-screen-audio"));
        assert!(stream_id_for(pid, Source::Screen, MediaKind::Video).contains("-screen-video"));
    }

    #[test]
    fn stream_id_for_only_uses_msid_safe_chars() {
        let pid = Uuid::new_v4();
        for &source in &ALL_SOURCES {
            for &kind in &ALL_KINDS {
                let id = stream_id_for(pid, source, kind);
                assert!(
                    id.chars().all(is_msid_safe),
                    "stream_id `{id}` contains a char str0m would strip (RFC 7941)"
                );
            }
        }
    }

    #[test]
    fn stream_id_for_deterministic_for_same_inputs() {
        let pid = Uuid::new_v4();
        assert_eq!(
            stream_id_for(pid, Source::Screen, MediaKind::Video),
            stream_id_for(pid, Source::Screen, MediaKind::Video)
        );
    }

    #[test]
    fn stream_id_for_unique_per_publisher() {
        let a = stream_id_for(Uuid::new_v4(), Source::Camera, MediaKind::Audio);
        let b = stream_id_for(Uuid::new_v4(), Source::Camera, MediaKind::Audio);
        assert_ne!(a, b);
    }
}
