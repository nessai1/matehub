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
        }
    }

    /// Main SFU event loop. Call this from a spawned tokio task.
    pub async fn run(mut self) {
        let mut buf = vec![0u8; 2000];
        let mut interval = tokio::time::interval(Duration::from_millis(5));

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
                        Some(cmd) => self.handle_command(cmd),
                        None => {
                            tracing::info!("SFU command channel closed, shutting down");
                            break;
                        }
                    }
                }

                // 3. Timer tick -- drive all Rtc instances forward
                _ = interval.tick() => {
                    self.tick();
                }
            }

            // After any event, poll all Rtc instances for outputs
            self.poll_all_outputs().await;
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
            .flat_map(|p| {
                p.tracks_in
                    .iter()
                    .map(|t| (p.id, t.mid, t.kind))
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
        };
        session.participants.insert(participant_id, participant);

        // Send SDP answer
        let _ = reply_tx.send(ServerMessage::Answer {
            sdp_answer: answer_str,
            participant_id,
        });

        // Set up track forwarding: new participant needs to send existing tracks
        // This will happen via renegotiation when we detect MediaAdded events

        // For existing participants: they need to add outgoing tracks for the new participant
        // This happens when we get MediaAdded events from the new participant's Rtc

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
                // Mark negotiating tracks as open
                for track in &mut participant.tracks_out {
                    if let TrackOutState::Negotiating(mid) = track.state {
                        track.state = TrackOutState::Open(mid);
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
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return;
        };
        let Some(participant) = session.participants.get_mut(&participant_id) else {
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

        // TODO: renegotiate with remaining participants to remove tracks

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
                destination: self.local_addr,
                contents,
            },
        );

        // Find which Rtc accepts this packet
        for session in self.sessions.values_mut() {
            for participant in session.participants.values_mut() {
                if participant.rtc.accepts(&input) {
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
        for session in self.sessions.values_mut() {
            for participant in session.participants.values_mut() {
                // Drive time forward even if ICE is disconnected --
                // it might recover when STUN packets arrive
                let _ = participant.rtc.handle_input(Input::Timeout(now));
            }
        }
        // Don't clean up here -- participants are removed via SfuCommand::Leave
        // triggered by WebSocket close
    }

    async fn poll_all_outputs(&mut self) {
        // Collect media data to propagate (can't borrow mutably twice)
        let mut media_to_forward: Vec<(SessionId, ParticipantId, MediaData)> = Vec::new();
        let mut tracks_opened: Vec<(SessionId, ParticipantId, Mid, MediaKind)> = Vec::new();
        let mut negotiations_needed: Vec<(SessionId, ParticipantId)> = Vec::new();

        for (session_id, session) in &mut self.sessions {
            for (pid, participant) in &mut session.participants {
                if !participant.rtc.is_alive() {
                    continue;
                }

                loop {
                    match participant.rtc.poll_output() {
                        Ok(Output::Transmit(transmit)) => {
                            if let Err(e) = self
                                .udp_socket
                                .send_to(&transmit.contents, transmit.destination)
                                .await
                            {
                                tracing::warn!("UDP send error: {e}");
                            }
                        }
                        Ok(Output::Event(event)) => match event {
                            Event::IceConnectionStateChange(state) => {
                                tracing::info!(%pid, ?state, "ICE state changed");
                                // Don't disconnect on ICE Disconnected -- it can recover.
                                // Only disconnect on explicit close or fatal error.
                            }
                            Event::MediaAdded(e) => {
                                tracing::info!(%pid, mid = %e.mid, kind = ?e.kind, "media track added");
                                participant.tracks_in.push(TrackIn {
                                    mid: e.mid,
                                    kind: e.kind,
                                });
                                tracks_opened.push((*session_id, *pid, e.mid, e.kind));
                            }
                            Event::MediaData(data) => {
                                media_to_forward.push((*session_id, *pid, data));
                            }
                            Event::KeyframeRequest(req) => {
                                // Forward keyframe request to the source
                                if let Some(mut writer) = participant.rtc.writer(req.mid) {
                                    let _ = writer.request_keyframe(req.rid, req.kind);
                                }
                            }
                            _ => {}
                        },
                        Ok(Output::Timeout(_)) => break,
                        Err(e) => {
                            tracing::warn!(%pid, "poll_output error: {e}");
                            participant.rtc.disconnect();
                            break;
                        }
                    }
                }
            }
        }

        // Forward media data
        for (session_id, origin_pid, data) in &media_to_forward {
            let Some(session) = self.sessions.get_mut(session_id) else {
                continue;
            };

            // Find outgoing mid for each other participant
            for (pid, participant) in &mut session.participants {
                if pid == origin_pid {
                    continue;
                }

                // Find the track_out that maps to this origin + mid
                let out_mid = participant.tracks_out.iter().find_map(|t| {
                    if t.origin == *origin_pid && t.origin_mid == data.mid {
                        t.open_mid()
                    } else {
                        None
                    }
                });

                let Some(mid) = out_mid else {
                    continue;
                };

                let Some(writer) = participant.rtc.writer(mid) else {
                    continue;
                };

                let Some(pt) = writer.match_params(data.params) else {
                    continue;
                };

                if let Err(e) = writer.write(pt, data.network_time, data.time, data.data.clone()) {
                    tracing::warn!(%pid, "media write error: {e}");
                    participant.rtc.disconnect();
                }
            }
        }

        // Handle new tracks: add outgoing tracks to all other participants
        for (session_id, origin_pid, mid, kind) in &tracks_opened {
            let Some(session) = self.sessions.get_mut(session_id) else {
                continue;
            };

            for (pid, participant) in &mut session.participants {
                if pid == origin_pid {
                    continue;
                }

                // Check if we already have this track_out
                let already_has = participant.tracks_out.iter().any(|t| {
                    t.origin == *origin_pid && t.origin_mid == *mid
                });

                if !already_has {
                    participant.tracks_out.push(TrackOut {
                        origin: *origin_pid,
                        origin_mid: *mid,
                        kind: *kind,
                        state: TrackOutState::ToOpen,
                    });
                    negotiations_needed.push((*session_id, *pid));
                }
            }
        }

        // Negotiate new tracks
        for (session_id, pid) in negotiations_needed {
            let Some(session) = self.sessions.get_mut(&session_id) else {
                continue;
            };
            let Some(participant) = session.participants.get_mut(&pid) else {
                continue;
            };

            if participant.pending_offer.is_some() {
                continue; // Already negotiating
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
