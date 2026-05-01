pub mod session;

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam::channel::{Receiver, TryRecvError};
use str0m::change::SdpOffer;
use str0m::ice::IceCreds;
use str0m::media::{Direction, MediaData, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::signaling::ServerMessage;

pub use session::{SfuParticipant, SfuSession, Source, TrackIn, TrackOut, TrackOutState};

/// Drop-on-full send to a per-WS channel. Same intent as the helper in
/// api/ws.rs — slow consumers don't back-pressure the media thread. Kept
/// here as a free fn (not a method) so call sites inside partial-borrow
/// scopes work.
#[inline]
fn ws_try_send(tx: &mpsc::Sender<ServerMessage>, msg: ServerMessage) {
    if let Err(e) = tx.try_send(msg) {
        if matches!(e, mpsc::error::TrySendError::Full(_)) {
            metrics::counter!("matehub_video_ws_send_drops_total").increment(1);
        }
    }
}

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
        // Bounded tokio mpsc. The media thread uses `try_send` (sync, no
        // async runtime needed) so it can talk to the tokio side without
        // blocking; on Full we drop+counter — slow consumers don't get to
        // back-pressure media forwarding.
        reply_tx: mpsc::Sender<ServerMessage>,
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
    /// How long an ICE-disconnected participant can be silent before we
    /// declare them dead. Configurable from main; see config::Config.
    zombie_timeout: Duration,
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
    /// Local-ICE-ufrag → participant. Populated on Join, removed on Leave.
    /// First UDP packet from a peer is a STUN binding request whose
    /// USERNAME attribute is `{remote_ufrag}:{local_ufrag}`. We parse the
    /// local half and route to that exact participant — replaces an O(N)
    /// linear scan over every Rtc that turned the slow path into a DoS
    /// vector (anyone who knew the port could feed mostly-junk and burn
    /// CPU on `rtc.accepts()` calls).
    ufrag_to_participant: HashMap<String, (SessionId, ParticipantId)>,
    /// (session, participant) pairs that have at least one TrackOut in ToOpen
    /// state and need a server-initiated offer. Replaces a boolean+sweep:
    /// negotiate_pending_tracks() now iterates only the entries here, so a
    /// 500-participant session with two pending joins runs O(2) instead of
    /// O(500) per pass.
    pending_negotiation: HashSet<(SessionId, ParticipantId)>,
    /// Media forwarding stats (logged periodically)
    stats_audio_fwd: u64,
    stats_video_fwd: u64,
    /// UDP sends that returned WouldBlock — packet dropped to keep the
    /// media thread responsive. A non-zero rate here is a signal that the
    /// kernel send buffer (default 2MB, see main.rs) is too small or the
    /// network is saturated. Logged with the periodic stats line.
    stats_udp_drops: u64,
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

/// STUN magic cookie (RFC 5389 §6).
const STUN_MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];
/// STUN attribute type for USERNAME (RFC 5389 §15.3).
const STUN_ATTR_USERNAME: u16 = 0x0006;

/// Cheap STUN binding-request shape check. Returns true only when the
/// header looks STUN-ish — top 2 bits of byte 0 zero (per RFC 5389 §6:
/// "the most significant 2 bits of every STUN message MUST be zeroes")
/// and magic cookie at offset 4. Drops random noise (RTP/RTCP/DTLS look
/// nothing like this) before we burn cycles on the per-Rtc accept probe.
fn looks_like_stun(data: &[u8]) -> bool {
    data.len() >= 20
        && (data[0] & 0xC0) == 0
        && data[4..8] == STUN_MAGIC_COOKIE
}

/// Best-effort extraction of the local ufrag from a STUN binding request's
/// USERNAME attribute. Returns None on parser disagreement (truncated
/// packet, missing USERNAME, malformed UTF-8) — caller should fall through
/// to the existing slow scan rather than blackhole the packet.
fn parse_stun_local_ufrag(data: &[u8]) -> Option<&str> {
    if !looks_like_stun(data) {
        return None;
    }
    // Attributes start at offset 20 (header is 20 bytes).
    let mut p = 20usize;
    while p + 4 <= data.len() {
        let attr_type = u16::from_be_bytes([data[p], data[p + 1]]);
        let attr_len = u16::from_be_bytes([data[p + 2], data[p + 3]]) as usize;
        let value_start = p + 4;
        let value_end = value_start.checked_add(attr_len)?;
        if value_end > data.len() {
            return None;
        }
        if attr_type == STUN_ATTR_USERNAME {
            let val = &data[value_start..value_end];
            // RFC 8445 §7: USERNAME = "{remote_ufrag}:{local_ufrag}".
            let colon = val.iter().position(|&b| b == b':')?;
            return std::str::from_utf8(&val[colon + 1..]).ok();
        }
        // Pad to next 4-byte boundary; saturating to length avoids loops on
        // malformed lengths.
        p = value_end + ((4 - (attr_len % 4)) % 4);
    }
    None
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
        zombie_timeout: Duration,
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
            zombie_timeout,
            sessions: HashMap::new(),
            udp_socket,
            local_addr,
            candidate_addrs,
            cmd_rx,
            addr_to_participant: HashMap::new(),
            ufrag_to_participant: HashMap::new(),
            pending_negotiation: HashSet::new(),
            stats_audio_fwd: 0,
            stats_video_fwd: 0,
            stats_udp_drops: 0,
            stats_last_log: Instant::now(),
        }
    }

    /// Non-blocking UDP send. Drops the packet on WouldBlock instead of
    /// stalling the media thread on a full kernel send buffer.
    ///
    /// Static method (takes &UdpSocket / &mut u64 separately) so it can be
    /// called from inside a `forward_media_now` loop where `&mut self` is
    /// already partially borrowed by `session.participants.get_mut(...)`.
    /// MSG_DONTWAIT is a per-call non-blocking flag — the socket itself
    /// stays blocking for `recv_from`, which we still need for the
    /// SO_RCVTIMEO-driven tick cadence.
    #[inline]
    fn udp_send_one(
        socket: &UdpSocket,
        data: &[u8],
        dest: SocketAddr,
        drop_counter: &mut u64,
    ) {
        let sock = socket2::SockRef::from(socket);
        let addr: socket2::SockAddr = dest.into();
        match sock.send_to_with_flags(data, &addr, libc::MSG_DONTWAIT) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                *drop_counter += 1;
            }
            Err(e) => {
                // Non-WouldBlock errors (EHOSTUNREACH on transient route flaps,
                // EMSGSIZE on a way-too-big payload, etc). Trace level so we
                // don't flood the log; the periodic stats line surfaces volume.
                tracing::trace!(error = %e, %dest, "UDP send error");
            }
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
                    let elapsed = self.stats_last_log.elapsed().as_secs_f32();
                    if self.stats_audio_fwd > 0
                        || self.stats_video_fwd > 0
                        || self.stats_udp_drops > 0
                    {
                        tracing::info!(
                            audio_pps = (self.stats_audio_fwd as f32 / elapsed) as u32,
                            video_pps = (self.stats_video_fwd as f32 / elapsed) as u32,
                            udp_drops = self.stats_udp_drops,
                            "media forwarding stats"
                        );
                    }
                    // Surface drop volume to Prometheus too — the warn-on-each
                    // log line is gone, this is the new alertable signal.
                    if self.stats_udp_drops > 0 {
                        metrics::counter!("matehub_video_udp_send_drops_total")
                            .increment(self.stats_udp_drops);
                    }
                    self.stats_audio_fwd = 0;
                    self.stats_video_fwd = 0;
                    self.stats_udp_drops = 0;
                    self.stats_last_log = Instant::now();
                }

                // Skip missed ticks instead of bursting — if we lagged behind
                // (e.g. a big keyframe cascade), snap forward rather than
                // firing tick() several times in a row.
                next_tick = now + TICK;
            }

            // --- 4. Negotiation (set-gated) ---------------------------------
            if !self.pending_negotiation.is_empty() {
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

        // A pending server-side offer means we can't apply this client offer
        // right now — it's relative to the last stable SDP state. Stash it
        // (overwriting any previous queued offer; the latest reflects the
        // client's current PC) and drain inside handle_answer once
        // pending_offer clears.
        if participant.pending_offer.is_some() {
            if participant.queued_client_offer.is_some() {
                tracing::warn!(
                    %participant_id,
                    "replacing already-queued client offer with newer one"
                );
            } else {
                tracing::debug!(
                    %participant_id,
                    "queueing client offer until server offer is answered"
                );
            }
            participant.queued_client_offer = Some(sdp_offer);
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
        ws_try_send(
            &participant.ws_tx,
            ServerMessage::Answer {
                sdp_answer: answer_str,
                participant_id,
            },
        );

        tracing::info!(%participant_id, "client offer accepted, answer sent");
        Some(PollTarget::One(session_id, participant_id))
    }

    fn handle_join(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
        user_id: String,
        sdp_offer: String,
        reply_tx: mpsc::Sender<ServerMessage>,
    ) -> Option<PollTarget> {
        // Reconnect: the WS handler derives participant_id from
        // (session_id, user_id), so a fresh socket from the same user
        // lands on the same pid. If a participant entry already exists,
        // it's a stale Rtc from a dropped connection — tear it down via
        // the existing handle_leave path (which cleans tracks, fan-out,
        // and renegotiates remaining peers) before we slot the new Rtc in.
        let already_present = self
            .sessions
            .get(&session_id)
            .map(|s| s.participants.contains_key(&participant_id))
            .unwrap_or(false);
        if already_present {
            tracing::info!(
                %session_id,
                %participant_id,
                "reconnect detected — replacing stale participant"
            );
            self.handle_leave(session_id, participant_id);
        }

        // Parse the SDP offer (try JSON first, then raw SDP string)
        let offer: SdpOffer = match serde_json::from_str(&sdp_offer) {
            Ok(o) => o,
            Err(_) => match SdpOffer::from_sdp_string(&sdp_offer) {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!(%participant_id, "invalid SDP offer: {e}");
                    ws_try_send(
                        &reply_tx,
                        ServerMessage::Error {
                            message: format!("invalid SDP offer: {e}"),
                        },
                    );
                    return None;
                }
            },
        };

        // Create Rtc instance with explicit ICE creds so we know the local
        // ufrag before the first STUN binding. We track ufrag → participant
        // for O(1) UDP demux on first contact (see handle_udp_packet's
        // STUN routing).
        // Note: full ICE (not ICE-lite). ICE-lite causes immediate Disconnected
        // because str0m expects STUN before poll_output runs the first timeout.
        let creds = IceCreds::new();
        let local_ufrag = creds.ufrag.clone();
        let mut rtc = Rtc::builder().set_local_ice_credentials(creds).build();

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
                ws_try_send(
                    &reply_tx,
                    ServerMessage::Error {
                        message: format!("SDP negotiation failed: {e}"),
                    },
                );
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
            queued_client_offer: None,
            pending_source_hints: HashMap::new(),
            last_activity_at: Instant::now(),
            ice_disconnected: false,
        };
        session.participants.insert(participant_id, participant);

        // Send SDP answer
        ws_try_send(
            &reply_tx,
            ServerMessage::Answer {
                sdp_answer: answer_str,
                participant_id,
            },
        );

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
            self.pending_negotiation.insert((session_id, participant_id));
        }

        // Register ufrag for fast-path routing. Done last so we don't add
        // an entry for a participant that ended up not getting inserted.
        self.ufrag_to_participant
            .insert(local_ufrag, (session_id, participant_id));

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
        // pending_offer is now None — drain a queued client offer if there
        // was one. Re-fetching the participant here (instead of holding the
        // earlier &mut over the recursive call) keeps the borrow checker
        // happy and naturally handles the participant-vanished race.
        let queued = self
            .sessions
            .get_mut(&session_id)
            .and_then(|s| s.participants.get_mut(&participant_id))
            .and_then(|p| p.queued_client_offer.take());
        if let Some(sdp) = queued {
            tracing::info!(
                %participant_id,
                "draining queued client offer after server offer answered"
            );
            self.handle_client_offer(session_id, participant_id, sdp);
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
        // Same for ufrag map (linear over a typically-small set).
        self.ufrag_to_participant
            .retain(|_, &mut (sid, pid)| !(sid == session_id && pid == participant_id));

        // Drop any pending-negotiation entry tied to this participant — they
        // can't ack our offer if they're gone.
        self.pending_negotiation
            .remove(&(session_id, participant_id));

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
                    ws_try_send(
                        &p.ws_tx,
                        ServerMessage::Offer {
                            sdp_offer: offer_str,
                            tracks: None,
                        },
                    );
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
            self.ufrag_to_participant
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

        // Slow path. Two stages:
        //   1. Cheap shape check — drop random UDP noise (RTP/RTCP/DTLS look
        //      nothing like STUN). The rest of this method only runs for
        //      packets that look like ICE binding requests.
        //   2. Parse USERNAME, look up our ufrag → participant. If found,
        //      route directly. The previous N×M scan over every Rtc is now
        //      a single lookup; legitimate first-contacts hit it once and
        //      the addr cache takes over from packet 2 onward.
        if !looks_like_stun(data) {
            metrics::counter!("matehub_video_unknown_source_drops_total").increment(1);
            return None;
        }
        let Some(ufrag) = parse_stun_local_ufrag(data) else {
            // STUN-shaped but no parseable USERNAME — could be a binding
            // success response from somewhere (legitimate when we initiate
            // checks) or junk. Drop; caller doesn't lose anything because
            // we wouldn't have known where to route it anyway.
            return None;
        };
        let Some(&(session_id, participant_id)) =
            self.ufrag_to_participant.get(ufrag)
        else {
            metrics::counter!("matehub_video_unknown_ufrag_drops_total").increment(1);
            return None;
        };

        let now = Instant::now();
        let destination = self.primary_candidate_addr();
        let session = self.sessions.get_mut(&session_id)?;
        let participant = session.participants.get_mut(&participant_id)?;

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

        if !participant.rtc.accepts(&input) {
            // Ufrag matched but Rtc still rejects — could be a stale STUN
            // request from a defunct ICE pair, or a malformed body. One
            // drop, no escalation.
            return None;
        }
        participant.last_activity_at = now;
        let pid = participant.id;
        if let Err(e) = participant.rtc.handle_input(input) {
            tracing::warn!(id = %pid, "rtc handle_input error: {e}");
            participant.rtc.disconnect();
            return None;
        }
        // Cache addr → participant for O(1) demux on subsequent packets.
        self.addr_to_participant
            .insert(source, (session_id, pid));
        Some((session_id, pid))
    }

    fn tick(&mut self) {
        let now = Instant::now();

        // Refresh top-K audio publishers per session. O(audio_tracks) work
        // every 20ms — negligible even at 500 participants where the audio
        // publisher list is at most a few hundred.
        for session in self.sessions.values_mut() {
            session.recompute_top_audio();
        }

        // Collect zombies: ICE disconnected + no media for the configured
        // window. At 500 participants this is a single O(N) scan per tick.
        let mut zombies: Vec<(SessionId, ParticipantId)> = Vec::new();

        for (session_id, session) in &mut self.sessions {
            for participant in session.participants.values_mut() {
                let _ = participant.rtc.handle_input(Input::Timeout(now));

                if participant.ice_disconnected
                    && participant.last_activity_at.elapsed() > self.zombie_timeout
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

    /// Drain outputs for one participant. Wraps the inner loop in
    /// `catch_unwind` so a panic deep in str0m (malformed RTCP, codec edge
    /// case, etc.) terminates *that* participant cleanly instead of bringing
    /// the entire media thread down — and with it, every active call. The
    /// affected Rtc is marked dead and the next zombie sweep will collect it.
    fn poll_participant(&mut self, session_id: SessionId, source_pid: ParticipantId) {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.poll_participant_inner(session_id, source_pid);
        }));
        if outcome.is_err() {
            metrics::counter!("matehub_video_panics_caught_total").increment(1);
            tracing::error!(
                %session_id,
                %source_pid,
                "caught panic in poll_participant — marking participant dead"
            );
            if let Some(session) = self.sessions.get_mut(&session_id)
                && let Some(p) = session.participants.get_mut(&source_pid)
            {
                p.rtc.disconnect();
                p.ice_disconnected = true;
                // Backdate so the next zombie tick collects it immediately
                // (no point waiting another 12s for a poisoned Rtc).
                p.last_activity_at = Instant::now()
                    .checked_sub(self.zombie_timeout + Duration::from_secs(1))
                    .unwrap_or(p.last_activity_at);
                ws_try_send(
                    &p.ws_tx,
                    ServerMessage::Error {
                        message: "internal SFU error — please rejoin".into(),
                    },
                );
            }
        }
    }

    /// Inner body — separated so `catch_unwind` can wrap it without nesting.
    fn poll_participant_inner(
        &mut self,
        session_id: SessionId,
        source_pid: ParticipantId,
    ) {
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
                    Self::udp_send_one(
                        &self.udp_socket,
                        &transmit.contents,
                        transmit.destination,
                        &mut self.stats_udp_drops,
                    );
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
                            let source = session
                                .participants
                                .get_mut(&source_pid)
                                .and_then(|p| {
                                    p.pending_source_hints
                                        .get_mut(&e.kind)
                                        .and_then(|q| q.pop_front())
                                })
                                .unwrap_or(Source::Camera);
                            let mut newly_queued: Vec<ParticipantId> = Vec::new();
                            for (pid, p) in &mut session.participants {
                                if *pid == source_pid {
                                    p.tracks_in.push(TrackIn {
                                        mid: e.mid,
                                        kind: e.kind,
                                        source,
                                        seen_high_layer: false,
                                        // Initialise mid-loud (~64 / 127) so a
                                        // brand-new audio publisher rides into
                                        // top-K on their first packet, before
                                        // the EMA has had a chance to settle.
                                        // Without this, joiners are silenced
                                        // for a tick or two — sounds like an
                                        // audio cutout.
                                        audio_loudness_ema: 64.0,
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
                                        newly_queued.push(*pid);
                                    }
                                }
                            }
                            for pid in newly_queued {
                                self.pending_negotiation.insert((session_id, pid));
                            }
                        }
                    }
                    Event::MediaData(data) => {
                        // Stats counter folded into forward_media_now to avoid
                        // a second tracks_in scan — kind is already derived
                        // there as part of the simulcast filter.
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
        // Single TrackIn lookup serves four purposes: stats (kind),
        // simulcast filter (seen_high_layer), audio EMA update for the
        // top-K speaker selector, and a hard "unknown track" gate.
        //
        // Simulcast filter (steady state forwards `h`, start-up fallback
        // to `l` until the high layer's first packet arrives):
        //   * rid == None → no simulcast, always forward (audio mostly).
        //   * rid == "h"  → forward + flip seen_high_layer.
        //   * rid != "h"  → forward only while seen_high_layer == false.
        //
        // Audio loudness EMA (RFC 6464): voice_activity bit gates whether
        // the level moves the average up or just decays. Smoothing prevents
        // a single loud burst from kicking a quiet participant into the
        // top-K and then back out 20ms later.
        let (kind, can_forward, in_top_audio) = {
            let Some(session) = self.sessions.get_mut(&session_id) else {
                return;
            };
            let in_top = session
                .top_audio_publishers
                .contains(&(source_pid, data.mid));
            let Some(p) = session.participants.get_mut(&source_pid) else {
                return;
            };
            let Some(track) = p.tracks_in.iter_mut().find(|t| t.mid == data.mid)
            else {
                return;
            };
            let kind = track.kind;
            let can_forward = match data.rid.as_ref().map(|r| &**r as &str) {
                None => true,
                Some("h") => {
                    if !track.seen_high_layer {
                        track.seen_high_layer = true;
                        tracing::info!(
                            publisher = %source_pid,
                            mid = %data.mid,
                            "high simulcast layer live — fallback retired"
                        );
                    }
                    true
                }
                Some(_) => !track.seen_high_layer,
            };
            // Audio loudness bookkeeping. Only audible packets push the
            // EMA upward; silence (voice_activity=false) decays it so
            // newly-loud speakers can claim a top-K slot quickly.
            if kind == MediaKind::Audio {
                let voice_active = data.ext_vals.voice_activity == Some(true);
                if let Some(level_dbov) = data.ext_vals.audio_level {
                    if voice_active {
                        // -127..0 dBov → 0..127 loudness; smoothing 0.7/0.3.
                        let loud = (-level_dbov) as f32;
                        track.audio_loudness_ema =
                            track.audio_loudness_ema * 0.7 + loud * 0.3;
                    } else {
                        track.audio_loudness_ema *= 0.7;
                    }
                } else {
                    // Extension wasn't negotiated / publisher dropped it —
                    // can't do better than treating every packet as
                    // "speaking", which keeps the participant in top-K.
                    // Fine; degrades gracefully to forward-everything.
                    track.audio_loudness_ema = track.audio_loudness_ema.max(64.0);
                }
            }
            (kind, can_forward, in_top)
        };

        // Counters reflect input volume from publishers (pre-filter), same
        // semantic as before this method owned the increment.
        match kind {
            MediaKind::Audio => self.stats_audio_fwd += 1,
            MediaKind::Video => self.stats_video_fwd += 1,
        }

        if !can_forward {
            return;
        }

        // Top-K audio filter: drop audio from publishers not currently in
        // the speaker set. The set is recomputed on every tick; while it's
        // small enough (≤ K publishers) recompute keeps everyone in,
        // making this branch a no-op for typical 2-5 person calls.
        if kind == MediaKind::Audio && !in_top_audio {
            return;
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
                        Self::udp_send_one(
                            &self.udp_socket,
                            &t.contents,
                            t.destination,
                            &mut self.stats_udp_drops,
                        );
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
    /// Gated by `self.pending_negotiation` — only iterates participants that
    /// pushed a ToOpen track since the last pass. Re-inserts on the
    /// pending-offer-still-set retry path.
    fn negotiate_pending_tracks(&mut self) {
        // Take the set so we own it; participants that re-queue (pending_offer
        // still set) re-insert themselves below for the next tick to pick up.
        let to_negotiate: Vec<(SessionId, ParticipantId)> =
            std::mem::take(&mut self.pending_negotiation).into_iter().collect();

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
                // answer. Re-insert into the pending set so the next tick
                // retries after handle_answer clears pending_offer.
                self.pending_negotiation.insert((session_id, pid));
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

                    ws_try_send(
                        &participant.ws_tx,
                        ServerMessage::Offer {
                            sdp_offer: offer_str,
                            tracks: if track_mappings.is_empty() {
                                None
                            } else {
                                Some(track_mappings.clone())
                            },
                        },
                    );
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

    // ── STUN early-drop & USERNAME parsing ─────────────────────────

    fn build_stun_binding_with_username(username: &[u8]) -> Vec<u8> {
        let attr_pad = (4 - (username.len() % 4)) % 4;
        let attrs_len = 4 + username.len() + attr_pad;
        let mut buf = Vec::with_capacity(20 + attrs_len);
        // Type: Binding Request (0x0001)
        buf.extend_from_slice(&0x0001u16.to_be_bytes());
        // Length: attrs_len
        buf.extend_from_slice(&(attrs_len as u16).to_be_bytes());
        // Magic cookie
        buf.extend_from_slice(&STUN_MAGIC_COOKIE);
        // Transaction id (12 bytes; arbitrary)
        buf.extend_from_slice(&[0xAA; 12]);
        // USERNAME attribute
        buf.extend_from_slice(&STUN_ATTR_USERNAME.to_be_bytes());
        buf.extend_from_slice(&(username.len() as u16).to_be_bytes());
        buf.extend_from_slice(username);
        buf.extend(std::iter::repeat_n(0u8, attr_pad));
        buf
    }

    #[test]
    fn looks_like_stun_accepts_valid_binding() {
        let pkt = build_stun_binding_with_username(b"remote:local");
        assert!(looks_like_stun(&pkt));
    }

    #[test]
    fn looks_like_stun_rejects_short_packet() {
        let short = vec![0u8; 19];
        assert!(!looks_like_stun(&short));
    }

    #[test]
    fn looks_like_stun_rejects_rtp_shape() {
        // Real RTP packets have version=2 in top 2 bits of byte 0 → 0x80.
        let mut rtp = vec![0u8; 30];
        rtp[0] = 0x80;
        // Magic cookie absent; this is exactly the noise we want to drop.
        assert!(!looks_like_stun(&rtp));
    }

    #[test]
    fn looks_like_stun_rejects_wrong_magic() {
        let mut pkt = build_stun_binding_with_username(b"remote:local");
        // Corrupt the cookie.
        pkt[4] = 0xFF;
        assert!(!looks_like_stun(&pkt));
    }

    #[test]
    fn parse_stun_local_ufrag_returns_segment_after_colon() {
        let pkt = build_stun_binding_with_username(b"remote_uf:my_local");
        assert_eq!(parse_stun_local_ufrag(&pkt), Some("my_local"));
    }

    #[test]
    fn parse_stun_local_ufrag_handles_padding_correctly() {
        // 3-byte username forces 1 byte of padding.
        let pkt = build_stun_binding_with_username(b"a:b");
        assert_eq!(parse_stun_local_ufrag(&pkt), Some("b"));
    }

    #[test]
    fn parse_stun_local_ufrag_returns_none_when_missing_colon() {
        let pkt = build_stun_binding_with_username(b"no-colon-here");
        assert_eq!(parse_stun_local_ufrag(&pkt), None);
    }

    #[test]
    fn parse_stun_local_ufrag_returns_none_for_non_stun() {
        // RTP-shaped: top byte 0x80, no magic.
        let mut rtp = vec![0u8; 60];
        rtp[0] = 0x80;
        assert_eq!(parse_stun_local_ufrag(&rtp), None);
    }

    #[test]
    fn parse_stun_local_ufrag_safe_against_truncation() {
        // Build a packet then cut off mid-attribute. Must not panic.
        let mut pkt = build_stun_binding_with_username(b"remote:local");
        pkt.truncate(22); // header + 2 bytes of attr type
        assert_eq!(parse_stun_local_ufrag(&pkt), None);
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
