pub mod session;

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam::channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use parking_lot::RwLock;
use str0m::bwe::BweKind;
use str0m::change::SdpOffer;
use str0m::ice::IceCreds;
use str0m::media::{Direction, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::rtp::RtpPacket;
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
    if let Err(e) = tx.try_send(msg)
        && matches!(e, mpsc::error::TrySendError::Full(_))
    {
        metrics::counter!("matehub_video_ws_send_drops_total").increment(1);
    }
}

pub type ParticipantId = Uuid;
pub type SessionId = Uuid;

/// Starting RTP sequence number for every rewritten egress stream. The 10_000
/// headroom absorbs ingress reorder that extends BELOW the first-arrived seq
/// (str0m's `extend_u16` misorder window is 100) without clamping; still well
/// under 65_536 so the first write has ROC 0 (str0m warns on non-zero-ROC
/// first `write_rtp`, and SRTP would need out-of-band ROC signalling).
const EGRESS_SEQ_START: u64 = 10_000;

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
    /// Pre-resolved UDP packet from the dispatcher thread. The dispatcher
    /// already turned `source_addr` into `(session_id, participant_id)`
    /// via the shared addr/ufrag maps, so the shard's only job is to feed
    /// it to the right Rtc — no map lookups, no slow-path scan.
    UdpInput {
        session_id: SessionId,
        participant_id: ParticipantId,
        source: SocketAddr,
        data: Vec<u8>,
    },
}

/// Where a UDP packet should land. Lives in the shared maps the dispatcher
/// reads on every datagram.
#[derive(Copy, Clone, Debug)]
pub struct ShardTarget {
    pub shard: usize,
    pub sid: SessionId,
    pub pid: ParticipantId,
}

/// Cross-thread routing tables shared between the dispatcher (reads) and
/// the shards (writes on Join/Leave). Behind RwLock because the dispatcher
/// reads constantly (every UDP packet) while shards write only on
/// participant lifecycle events — RwLock keeps the read path uncontended.
pub struct SharedMaps {
    /// Source-addr → shard target. Dispatcher's fast path. Populated by
    /// the dispatcher itself once a packet is successfully resolved via
    /// STUN ufrag, so subsequent packets from the same 4-tuple route
    /// directly without parsing.
    pub addr_to_target: RwLock<HashMap<SocketAddr, ShardTarget>>,
    /// Local-ICE-ufrag → shard target. Written by shards on Join, read by
    /// the dispatcher on first contact from a new peer (STUN binding
    /// USERNAME's `local_ufrag` half).
    pub ufrag_to_target: RwLock<HashMap<String, ShardTarget>>,
}

impl SharedMaps {
    pub fn new() -> Self {
        Self {
            addr_to_target: RwLock::new(HashMap::new()),
            ufrag_to_target: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for SharedMaps {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable shard assignment for a session id. `as_u128() % N` is the same
/// hash on every node — sessions live entirely on one shard, so there's
/// no cross-shard packet hop on the hot path.
fn shard_for_session(sid: SessionId, num_shards: usize) -> usize {
    if num_shards == 0 {
        0
    } else {
        (sid.as_u128() % num_shards as u128) as usize
    }
}

/// External handle to the SFU. Replaces the single-engine `Sender<SfuCommand>`
/// — this owns N shard senders and routes by session id.
///
/// Only carries what callers actually need (`shard_senders`). The UDP socket,
/// candidate addr, and shared maps live with the dispatcher / shard threads
/// after construction — keeping them on the pool would just be dead weight.
#[derive(Clone)]
pub struct SfuPool {
    /// One sender per shard. Index = shard number.
    shard_senders: Arc<Vec<Sender<SfuCommand>>>,
}

impl SfuPool {
    /// Route a command to the shard owning this session. On full channel
    /// the command is dropped + counter — same back-pressure semantics as
    /// the single-engine version.
    pub fn send(&self, sid: SessionId, cmd: SfuCommand) {
        let shard = shard_for_session(sid, self.shard_senders.len());
        if self.shard_senders[shard].try_send(cmd).is_err() {
            metrics::counter!("matehub_video_sfu_cmd_drops_total").increment(1);
        }
    }

    /// Test-only: build a pool wrapping externally-supplied senders, skipping
    /// the real media thread + dispatcher spawn. Caller is responsible for
    /// draining the receivers (typically with a mock that responds to Join).
    /// Lib-time dead-code analysis can't see integration tests in `tests/`,
    /// hence the explicit allow.
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn for_test(senders: Vec<Sender<SfuCommand>>) -> Self {
        Self {
            shard_senders: Arc::new(senders),
        }
    }

    /// Spawn `num_shards` media threads + a single UDP dispatcher thread.
    /// Returns a Pool wrapping the senders. Threads are leaked (not joined
    /// on drop) — they run for the process's lifetime, mirroring the
    /// previous single-thread engine's setup.
    pub fn spawn(
        num_shards: usize,
        udp_socket: Arc<UdpSocket>,
        public_ips: Vec<std::net::IpAddr>,
        zombie_timeout: Duration,
        cmd_buffer: usize,
    ) -> Self {
        assert!(num_shards >= 1, "need at least one media shard");
        let local_port = udp_socket.local_addr().expect("UDP socket bound").port();
        let candidate_addrs: Vec<SocketAddr> = public_ips
            .into_iter()
            .map(|ip| SocketAddr::new(ip, local_port))
            .collect();
        assert!(
            !candidate_addrs.is_empty(),
            "SfuPool needs at least one public IP for host candidates"
        );

        let shared_maps = Arc::new(SharedMaps::new());
        let mut senders = Vec::with_capacity(num_shards);

        for shard_index in 0..num_shards {
            let (tx, rx) = crossbeam::channel::bounded::<SfuCommand>(cmd_buffer);
            senders.push(tx);
            let shard = SfuShard::new(
                shard_index,
                Arc::clone(&udp_socket),
                candidate_addrs.clone(),
                rx,
                Arc::clone(&shared_maps),
                zombie_timeout,
            );
            let handle = std::thread::Builder::new()
                .name(format!("sfu-shard-{shard_index}"))
                .stack_size(2 * 1024 * 1024)
                .spawn(move || shard.run_blocking())
                .expect("spawn shard thread");
            std::mem::forget(handle);
        }

        let dispatcher_senders = senders.clone();
        let dispatcher = std::thread::Builder::new()
            .name("sfu-dispatcher".into())
            .spawn(move || dispatcher_loop(udp_socket, dispatcher_senders, shared_maps))
            .expect("spawn dispatcher thread");
        std::mem::forget(dispatcher);

        Self {
            shard_senders: Arc::new(senders),
        }
    }
}

/// UDP dispatcher loop. Single thread; reads every datagram, resolves it
/// to a (shard, sid, pid) target via the shared maps, and forwards via
/// the right shard's channel. Sub-microsecond per-packet on the hot path
/// (HashMap lookup + try_send), so this is not a meaningful bottleneck
/// even at hundreds of Mbps — the heavy lifting (SRTP, fan-out) happens
/// on the shard threads.
fn dispatcher_loop(
    socket: Arc<UdpSocket>,
    senders: Vec<Sender<SfuCommand>>,
    shared_maps: Arc<SharedMaps>,
) {
    const TICK: Duration = Duration::from_millis(50);
    if let Err(e) = socket.set_read_timeout(Some(TICK)) {
        tracing::error!("dispatcher set_read_timeout failed: {e}");
        return;
    }
    let mut buf = vec![0u8; 2000];
    loop {
        match socket.recv_from(&mut buf) {
            Ok((n, source)) => {
                let pkt = &buf[..n];
                // Cache fast path: addr already known → route directly.
                let target = shared_maps.addr_to_target.read().get(&source).copied();
                let target = if let Some(t) = target {
                    Some(t)
                } else {
                    // First contact from this addr. Must be a STUN binding
                    // request — anything else is junk we can drop without
                    // burning CPU.
                    if !looks_like_stun(pkt) {
                        metrics::counter!("matehub_video_unknown_source_drops_total").increment(1);
                        None
                    } else if let Some(ufrag) = parse_stun_local_ufrag(pkt) {
                        let resolved = shared_maps.ufrag_to_target.read().get(ufrag).copied();
                        if let Some(t) = resolved {
                            // Cache for subsequent packets from this 4-tuple.
                            shared_maps.addr_to_target.write().insert(source, t);
                        } else {
                            metrics::counter!("matehub_video_unknown_ufrag_drops_total")
                                .increment(1);
                        }
                        resolved
                    } else {
                        None
                    }
                };
                let Some(target) = target else { continue };
                let cmd = SfuCommand::UdpInput {
                    session_id: target.sid,
                    participant_id: target.pid,
                    source,
                    data: pkt.to_vec(),
                };
                if senders[target.shard].try_send(cmd).is_err() {
                    metrics::counter!("matehub_video_udp_dispatch_drops_total").increment(1);
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                // Periodic wake-up so the kernel timer drives this thread —
                // there's no other tick mechanism.
            }
            Err(e) => {
                tracing::error!("dispatcher recv error: {e}");
            }
        }
    }
}

/// One media-thread shard. Owns a disjoint set of sessions (assigned via
/// `shard_for_session`); never sees packets for sessions on other shards
/// because the dispatcher routes by session id. Ergo no inter-shard
/// locking on the hot path — each shard runs single-threaded over its own
/// state, like the pre-shard engine did over everything.
pub struct SfuShard {
    /// 0..N-1 — used so this shard can stamp `ShardTarget.shard` into the
    /// shared maps without consulting the pool.
    shard_index: usize,
    /// How long an ICE-disconnected participant can be silent before we
    /// declare them dead. Configurable from main; see config::Config.
    zombie_timeout: Duration,
    sessions: HashMap<SessionId, SfuSession>,
    /// Shared write end of the UDP socket. Sends are atomic at the kernel
    /// level so multiple shards writing concurrently is fine; we still go
    /// through `udp_send_one` for the MSG_DONTWAIT non-blocking semantics.
    udp_socket: Arc<UdpSocket>,
    /// Host candidate addresses advertised to every peer.
    /// Multiple IPs (wifi, ethernet, vpn) so ICE can pick whichever
    /// actually routes back to the server — avoids peer-reflexive
    /// fallback when the server is bound on 0.0.0.0 across interfaces.
    candidate_addrs: Vec<SocketAddr>,
    /// Per-shard command channel. UDP packets pre-resolved by the
    /// dispatcher arrive as `SfuCommand::UdpInput`; lifecycle events
    /// (Join/Leave/Answer/etc.) come from WS handlers via `SfuPool::send`.
    cmd_rx: Receiver<SfuCommand>,
    /// Routing tables shared with the dispatcher and (read-only) other
    /// shards. This shard writes its own (sid, pid) into them on Join,
    /// removes on Leave / session destroy.
    shared_maps: Arc<SharedMaps>,
    /// (session, participant) pairs that have at least one TrackOut in ToOpen
    /// state and need a server-initiated offer.
    pending_negotiation: HashSet<(SessionId, ParticipantId)>,
    /// Subscribers that got a `write_rtp` this event-loop iteration and need
    /// a `poll_output` pass to put the packet on the wire (streams are
    /// unpaced, so the next poll emits the Transmit). Drained by
    /// `flush_pending_writes` through the FULL event handler — never poll a
    /// subscriber's Rtc inline in the forward path, see that fn's doc.
    pending_flush: HashSet<(SessionId, ParticipantId)>,
    /// Media forwarding stats (logged periodically)
    stats_audio_fwd: u64,
    stats_video_fwd: u64,
    /// `write_rtp` failures on egress (per-subscriber write the target's
    /// str0m rejected). The sample-mode-era `discontig` counter is gone:
    /// `MediaData.contiguous` does not exist in rtp_mode, so it could only
    /// ever read zero — and its "zero means repacketization" interpretation
    /// guide would have hard-wired the wrong diagnosis. Ingress gaps are
    /// visible in the per-packet `video rtp in` trace instead.
    stats_video_write_err: u64,
    /// Round two of the artifact hunt. `written` = video frames handed to at
    /// least one subscriber's writer. Compare against `stats_video_fwd`
    /// (frames str0m emitted from publishers): a gap means the forward loop
    /// silently drops frames → the receiver decodes a broken reference chain
    /// (artifacts) with no wire loss. `no_writer`/`no_pt` attribute the gap:
    /// the subscriber transceiver had no writer (mid not ready) or no matching
    /// payload type. All skew-free (one process, one window).
    stats_video_written: u64,
    stats_video_no_writer: u64,
    stats_video_no_pt: u64,
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
    data.len() >= 20 && (data[0] & 0xC0) == 0 && data[4..8] == STUN_MAGIC_COOKIE
}

/// Best-effort extraction of the recipient-side (LOCAL, from our POV) ufrag
/// from a STUN binding request's USERNAME attribute.
///
/// Per RFC 8489 / RFC 8445 §7.2.2 and as documented in str0m's IceAgent:
/// USERNAME on a binding request from the remote peer is formatted as
/// `"{recipient_ufrag}:{sender_ufrag}"`. So when the SFU receives a
/// binding request, the FIRST half (before `:`) is its own ufrag — the
/// key we registered in `ufrag_to_target` on Join. The previous version
/// of this function returned the second half (sender's) and silently
/// dropped every STUN packet, breaking ICE entirely.
///
/// Returns None on parser disagreement (truncated packet, missing
/// USERNAME, malformed UTF-8). Caller drops the packet — the next
/// retransmit gives Chrome plenty of attempts.
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
            let colon = val.iter().position(|&b| b == b':')?;
            return std::str::from_utf8(&val[..colon]).ok();
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

impl SfuShard {
    fn new(
        shard_index: usize,
        udp_socket: Arc<UdpSocket>,
        candidate_addrs: Vec<SocketAddr>,
        cmd_rx: Receiver<SfuCommand>,
        shared_maps: Arc<SharedMaps>,
        zombie_timeout: Duration,
    ) -> Self {
        Self {
            shard_index,
            zombie_timeout,
            sessions: HashMap::new(),
            udp_socket,
            candidate_addrs,
            cmd_rx,
            shared_maps,
            pending_negotiation: HashSet::new(),
            pending_flush: HashSet::new(),
            stats_audio_fwd: 0,
            stats_video_fwd: 0,
            stats_video_write_err: 0,
            stats_video_written: 0,
            stats_video_no_writer: 0,
            stats_video_no_pt: 0,
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
    fn udp_send_one(socket: &UdpSocket, data: &[u8], dest: SocketAddr, drop_counter: &mut u64) {
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

    /// Per-shard event loop on a dedicated OS thread. Without UDP recv —
    /// the dispatcher does that and forwards pre-resolved packets via
    /// `SfuCommand::UdpInput`. Tick cadence comes from
    /// `cmd_rx.recv_timeout(TICK)` instead of socket SO_RCVTIMEO.
    pub fn run_blocking(mut self) {
        const TICK: Duration = Duration::from_millis(20);
        let mut next_tick = Instant::now() + TICK;
        tracing::info!(shard = self.shard_index, "SFU shard started");

        loop {
            let now = Instant::now();
            // recv_timeout returns Timeout when the deadline expires —
            // that's our tick driver. Saturating gives 0 if we're already
            // past next_tick so we don't block when behind.
            let timeout = next_tick.saturating_duration_since(now);

            let first = self.cmd_rx.recv_timeout(timeout);
            match first {
                Ok(cmd) => {
                    // Poll IMMEDIATELY after each command, before handling the
                    // next one. str0m's rtp_mode buffers an incoming RTP packet
                    // in a ONE-SLOT `pending_packet` (not a queue): a second
                    // `handle_input(Receive)` on the same Rtc before a poll
                    // silently overwrites the first packet — its seq is already
                    // registered (RR reports no loss!) but the payload never
                    // surfaces as Event::RtpPacket. Batch-handling a burst of
                    // datagrams then polling once dropped ~2/3 of ingress media
                    // (capture 7b302c3b: SFU RR packetsLost:0 while forwarding
                    // saw a third of the publisher's seq space).
                    let t = self.handle_command(cmd);
                    self.poll_target(t);
                    // Drain the rest non-blocking — better than waking
                    // up 50 times in a burst.
                    loop {
                        match self.cmd_rx.try_recv() {
                            Ok(cmd) => {
                                let t = self.handle_command(cmd);
                                self.poll_target(t);
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                tracing::info!(
                                    shard = self.shard_index,
                                    "shard cmd channel closed, exiting"
                                );
                                return;
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    tracing::info!(
                        shard = self.shard_index,
                        "shard cmd channel closed, exiting"
                    );
                    return;
                }
            }

            let now = Instant::now();
            if now >= next_tick {
                self.tick();
                // tick() pushed Input::Timeout into every Rtc; sweep to emit
                // any pending Transmit/Event.
                self.poll_all();

                if self.stats_last_log.elapsed() >= Duration::from_secs(5) {
                    let elapsed = self.stats_last_log.elapsed().as_secs_f32();
                    if self.stats_audio_fwd > 0
                        || self.stats_video_fwd > 0
                        || self.stats_udp_drops > 0
                    {
                        tracing::info!(
                            shard = self.shard_index,
                            audio_pps = (self.stats_audio_fwd as f32 / elapsed) as u32,
                            video_pps = (self.stats_video_fwd as f32 / elapsed) as u32,
                            video_write_err = self.stats_video_write_err,
                            // video_in vs video_out localises silent frame
                            // drops in the forward loop; no_writer/no_pt say why.
                            video_in = self.stats_video_fwd,
                            video_out = self.stats_video_written,
                            video_no_writer = self.stats_video_no_writer,
                            video_no_pt = self.stats_video_no_pt,
                            udp_drops = self.stats_udp_drops,
                            "media forwarding stats"
                        );
                    }
                    if self.stats_udp_drops > 0 {
                        metrics::counter!("matehub_video_udp_send_drops_total")
                            .increment(self.stats_udp_drops);
                    }
                    self.stats_audio_fwd = 0;
                    self.stats_video_fwd = 0;
                    self.stats_video_write_err = 0;
                    self.stats_video_written = 0;
                    self.stats_video_no_writer = 0;
                    self.stats_video_no_pt = 0;
                    self.stats_udp_drops = 0;
                    self.stats_last_log = Instant::now();
                }

                next_tick = now + TICK;
            }

            // Emit RTP forwarded during this iteration (targets loop and/or
            // tick's poll_all queued writes into subscribers' Rtcs).
            self.flush_pending_writes();

            if !self.pending_negotiation.is_empty() {
                self.negotiate_pending_tracks();
            }
        }
    }

    /// Drain outputs for whatever a just-handled command touched.
    fn poll_target(&mut self, target: Option<PollTarget>) {
        match target {
            Some(PollTarget::One(sid, pid)) => self.poll_participant(sid, pid),
            Some(PollTarget::Session(sid)) => self.poll_session(sid),
            None => {}
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
            SfuCommand::UdpInput {
                session_id,
                participant_id,
                source,
                data,
            } => self.handle_udp_input(session_id, participant_id, source, &data),
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
        // Reconnect / multi-tab collision: the WS handler derives
        // participant_id from (session_id, user_id), so a fresh socket
        // from the same user lands on the same pid. Reasons it might
        // collide:
        //   * Genuine reconnect after WS drop — old SfuParticipant is a
        //     stale Rtc; just tear it down.
        //   * Same user opens a second tab/window and joins the call —
        //     we do NOT want both tabs publishing audio. Tell the old
        //     tab "you got replaced" via WS so its UI exits the call,
        //     then proceed with the standard leave + new join.
        // The ForceDisconnected message gets pushed before handle_leave
        // because handle_leave drops the SfuParticipant (and with it
        // the ws_tx clone we'd otherwise need).
        let existing_ws_tx = self
            .sessions
            .get(&session_id)
            .and_then(|s| s.participants.get(&participant_id))
            .map(|p| p.ws_tx.clone());
        if let Some(tx) = existing_ws_tx {
            tracing::info!(
                %session_id,
                %participant_id,
                "join collision — kicking prior session for this user"
            );
            ws_try_send(
                &tx,
                ServerMessage::ForceDisconnected {
                    reason: "joined_elsewhere".into(),
                },
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
        // RTP mode: relay the publisher's original RTP payloads 1:1 instead of
        // depacketizing→repacketizing (str0m sample mode). Sample mode re-forms
        // the VP8/H264 bitstream on egress and corrupts inter-frame references
        // → cross-browser decode artifacts despite zero packet loss. In RTP
        // mode we forward `Event::RtpPacket` payloads verbatim via
        // `StreamTx::write_rtp`, so the bytes the encoder produced reach the
        // decoder untouched. This disables `Event::MediaData` / `Writer::write`.
        let mut rtc = Rtc::builder()
            .set_local_ice_credentials(creds)
            .set_rtp_mode(true)
            .build();

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
            .flat_map(|p| p.tracks_in.iter().map(|t| (p.id, t.mid, t.kind, t.source)))
            .collect();

        // Add participant
        let participant = SfuParticipant {
            id: participant_id,
            user_id,
            rtc,
            ws_tx: reply_tx.clone(),
            tracks_in: Vec::new(),
            tracks_out: Vec::new(),
            // Default high so subscribers start on `h` until BWE proves
            // otherwise — matches how Chrome behaves when fresh; switching
            // straight to `l` on join would cause a visible quality dip.
            egress_bitrate_bps: 5_000_000,
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
                    egress_seq_anchor: None,
                    egress_seq_high: 0,
                });
            }
            tracing::info!(
                %participant_id,
                existing = existing_tracks.len(),
                "queued existing tracks for new participant"
            );
            self.pending_negotiation
                .insert((session_id, participant_id));
        }

        // Register ufrag for fast-path routing. Done last so we don't add
        // an entry for a participant that ended up not getting inserted.
        // Lives in the SHARED map (read by the dispatcher on first
        // contact) — this shard's index is stamped into the target so
        // the dispatcher knows where to route subsequent packets.
        self.shared_maps.ufrag_to_target.write().insert(
            local_ufrag,
            ShardTarget {
                shard: self.shard_index,
                sid: session_id,
                pid: participant_id,
            },
        );

        tracing::info!(%session_id, %participant_id, shard = self.shard_index, "participant joined SFU");
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
                let mut to_register: Vec<((ParticipantId, Mid), (ParticipantId, Mid))> = Vec::new();

                for track in &mut participant.tracks_out {
                    if let TrackOutState::Negotiating(mid) = track.state {
                        track.state = TrackOutState::Open(mid);
                        if track.kind == MediaKind::Video {
                            newly_opened.push((track.origin, track.origin_mid));
                        }
                        to_register.push(((track.origin, track.origin_mid), (participant_id, mid)));
                    }
                }

                // Register fan-out entries (publishers → new subscriber).
                for (key, pair) in to_register {
                    session.forwarding_map.entry(key).or_default().push(pair);
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
        // Sweep both shared routing tables — addr cache and ufrag table —
        // of any entry targeting this participant. Done first so the
        // dispatcher stops sending us their packets; the per-Rtc cleanup
        // below can then proceed without a hot UDP source still feeding it.
        self.shared_maps
            .addr_to_target
            .write()
            .retain(|_, t| !(t.sid == session_id && t.pid == participant_id));
        self.shared_maps
            .ufrag_to_target
            .write()
            .retain(|_, t| !(t.sid == session_id && t.pid == participant_id));

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
            // Defensive sweep across the shared maps — any stragglers
            // (addr cache misses we never got around to retiring) need to
            // go too, otherwise the dispatcher would keep targeting a
            // shard whose session is gone.
            self.shared_maps
                .addr_to_target
                .write()
                .retain(|_, t| t.sid != session_id);
            self.shared_maps
                .ufrag_to_target
                .write()
                .retain(|_, t| t.sid != session_id);
            tracing::info!(%session_id, "SFU session destroyed (empty)");
            return None;
        }
        // Deactivation offers were queued for remaining participants — sweep
        // the session so those offers' transmits hit the wire.
        Some(PollTarget::Session(session_id))
    }

    /// Handle a UDP packet pre-resolved by the dispatcher. (sid, pid)
    /// already point at the right participant — we just feed it to the
    /// Rtc. No map lookups, no slow-path scans.
    fn handle_udp_input(
        &mut self,
        session_id: SessionId,
        participant_id: ParticipantId,
        source: SocketAddr,
        data: &[u8],
    ) -> Option<PollTarget> {
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
            return None;
        }
        participant.last_activity_at = now;
        let pid = participant.id;
        if let Err(e) = participant.rtc.handle_input(input) {
            tracing::warn!(id = %pid, "rtc handle_input error: {e}");
            participant.rtc.disconnect();
            return None;
        }
        Some(PollTarget::One(session_id, pid))
    }

    fn tick(&mut self) {
        let now = Instant::now();

        // Refresh top-K audio publishers per session. O(audio_tracks) work
        // every 20ms — negligible even at 500 participants where the audio
        // publisher list is at most a few hundred.
        for session in self.sessions.values_mut() {
            session.recompute_top_audio();
        }

        // NOTE: no adaptive layer pick here yet. A previous version ran
        // `next_layer` over every video TrackOut each tick and fired a PLI on
        // every l→h flip — but the forwarder ignores `selected_rid` in v1
        // (single-layer passthrough), so the only observable effect was a
        // spurious PLI per video mid ~5s after every subscription plus one per
        // BWE threshold crossing, taxing publishers with full I-frames for
        // switches that never happened. The machinery (`next_layer`,
        // `selected_rid`, BWE capture) stays for step 2, which will also make
        // the forwarder consume the decision.

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
        // Bug fix: previous version only called `p.rtc.disconnect()` and left
        // the participant in `session.participants`. Next tick the zombie
        // condition was still true (`ice_disconnected + elapsed > timeout`),
        // so the warning fired again — every 20ms forever, with no actual
        // cleanup. The fix is to run the full leave path: tears down the
        // Rtc, sweeps shared maps, deactivates m-lines on remaining peers,
        // destroys the session if it became empty.
        for (session_id, pid) in zombies {
            self.handle_leave(session_id, pid);
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

    /// Poll every participant that received a `write_rtp` in this event-loop
    /// iteration so the forwarded packet is emitted onto the wire (streams
    /// are unpaced — the poll yields the Transmit immediately).
    ///
    /// Goes through `poll_participant` (full event handling + catch_unwind),
    /// so the target's own pending events — its ingress `RtpPacket`s, its
    /// `KeyframeRequest`s — are processed, not discarded. Polling a flushed
    /// target can forward more packets and re-queue other targets (A→B
    /// forwards B's queued ingress → write to A → flush A); the cascade
    /// converges because each poll drains its Rtc to Timeout and no new
    /// input arrives mid-drain (single-threaded shard).
    fn flush_pending_writes(&mut self) {
        while let Some(&(session_id, pid)) = self.pending_flush.iter().next() {
            self.pending_flush.remove(&(session_id, pid));
            self.poll_participant(session_id, pid);
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
    fn poll_participant_inner(&mut self, session_id: SessionId, source_pid: ParticipantId) {
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
                        if let Some(session) = self.sessions.get_mut(&session_id)
                            && let Some(p) = session.participants.get_mut(&source_pid)
                        {
                            p.ice_disconnected = matches!(state, IceConnectionState::Disconnected);
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
                                    let already = p
                                        .tracks_out
                                        .iter()
                                        .any(|t| t.origin == source_pid && t.origin_mid == e.mid);
                                    if !already {
                                        p.tracks_out.push(TrackOut {
                                            origin: source_pid,
                                            origin_mid: e.mid,
                                            kind: e.kind,
                                            source,
                                            state: TrackOutState::ToOpen,
                                            egress_seq_anchor: None,
                                            egress_seq_high: 0,
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
                    Event::RtpPacket(pkt) => {
                        // Resolve which ingress track (mid, rid) this packet
                        // belongs to via its SSRC. str0m repairs RTX
                        // internally, so we only see media SSRCs here; an
                        // unmapped SSRC (e.g. a stream torn down mid-flight)
                        // is skipped.
                        let midrid = self
                            .sessions
                            .get_mut(&session_id)
                            .and_then(|s| s.participants.get_mut(&source_pid))
                            .and_then(|p| {
                                p.rtc
                                    .direct_api()
                                    .stream_rx(&pkt.header.ssrc)
                                    .map(|s| (s.mid(), s.rid()))
                            });
                        if let Some((mid, rid)) = midrid {
                            // Simulcast ingress gate: v1 forwards exactly ONE
                            // layer per mid. Screen share publishes two
                            // (`sendEncodings` rid h/l in the SDK); the
                            // forwarder resolves only to mid, so without this
                            // gate both layers' independent seq/timestamp
                            // spaces would interleave on the subscriber's
                            // single egress SSRC — undecodable at zero wire
                            // loss. Non-simulcast streams have no rid and pass
                            // as-is. Per-subscriber layer select is step 2.
                            let is_forwarded_layer = match rid {
                                None => true,
                                Some(rid) => &*rid == "h",
                            };
                            if is_forwarded_layer {
                                self.forward_rtp(session_id, source_pid, mid, &pkt);
                            }
                        }
                    }
                    Event::EgressBitrateEstimate(kind) => {
                        // BWE gives us per-Rtc throughput estimate; we feed
                        // it into the layer-pick decision in tick(). TWCC
                        // is the modern signal; REMB is legacy fallback for
                        // older endpoints. Both come in as `Bitrate` (bps).
                        let bps = match kind {
                            BweKind::Twcc(b) | BweKind::Remb(_, b) => b.as_u64(),
                        };
                        if let Some(session) = self.sessions.get_mut(&session_id)
                            && let Some(p) = session.participants.get_mut(&source_pid)
                        {
                            p.egress_bitrate_bps = bps;
                        }
                    }
                    Event::KeyframeRequest(req) => {
                        // Route PLI/FIR to the PUBLISHER, not the subscriber.
                        // req.mid is on the subscriber's Rtc. Find which TrackOut
                        // it belongs to and request from the origin — through
                        // the throttle so a 30-viewer fan-in coalesces into
                        // one keyframe instead of N.
                        if let Some(session) = self.sessions.get_mut(&session_id) {
                            let origin = session.participants.get(&source_pid).and_then(|p| {
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
    fn forward_rtp(
        &mut self,
        session_id: SessionId,
        source_pid: ParticipantId,
        source_mid: Mid,
        pkt: &RtpPacket,
    ) {
        // Single TrackIn lookup serves two purposes: stats (kind) and the
        // audio loudness EMA for the top-K speaker selector. Plus a hard
        // "unknown track" gate.
        //
        // Audio loudness EMA (RFC 6464), read from the RTP header extension:
        // voice_activity gates whether the level moves the average up or just
        // decays. Smoothing stops a single loud burst from kicking a quiet
        // participant into the top-K and back out 20ms later.
        //
        // (Simulcast layer selection is deliberately not here yet — v1 is
        // single-layer passthrough; per-subscriber `selected_rid` + RTP munging
        // arrive in step 2.)
        let (kind, in_top_audio, source_pp) = {
            let Some(session) = self.sessions.get_mut(&session_id) else {
                return;
            };
            let in_top = session
                .top_audio_publishers
                .contains(&(source_pid, source_mid));
            let Some(p) = session.participants.get_mut(&source_pid) else {
                return;
            };
            let Some(track) = p.tracks_in.iter_mut().find(|t| t.mid == source_mid) else {
                return;
            };
            let kind = track.kind;
            if kind == MediaKind::Audio {
                let voice_active = pkt.header.ext_vals.voice_activity == Some(true);
                if let Some(level_dbov) = pkt.header.ext_vals.audio_level {
                    if voice_active {
                        let loud = (-level_dbov) as f32;
                        track.audio_loudness_ema = track.audio_loudness_ema * 0.7 + loud * 0.3;
                    } else {
                        track.audio_loudness_ema *= 0.7;
                    }
                } else {
                    // Extension wasn't negotiated — treat every packet as
                    // "speaking" so we never starve an unannounced talker.
                    track.audio_loudness_ema = track.audio_loudness_ema.max(64.0);
                }
            }
            // Resolve the publisher's payload params for this packet's PT. Each
            // Rtc negotiates PTs independently with its browser, so the egress
            // PT differs from ingress — we must remap per subscriber (below),
            // not pass the PT through. `find` by pt gives us the source codec.
            let Some(pp) = p
                .rtc
                .codec_config()
                .find(|pp| pp.pt() == pkt.header.payload_type)
                .copied()
            else {
                // PT not in the publisher's negotiated set — nothing to map.
                return;
            };
            (kind, in_top, pp)
        };

        // Skip RTX (retransmission) packets: each leg runs its own RTX/NACK —
        // the subscriber's send stream re-derives retransmissions from its
        // cache (we mark video nackable), so relaying the publisher's RTX would
        // just be an unmappable PT. str0m repaired the ingress stream already.
        if source_pp.spec().codec == str0m::format::Codec::Rtx {
            return;
        }

        // Counters reflect input volume from publishers (pre-filter).
        match kind {
            MediaKind::Audio => self.stats_audio_fwd += 1,
            MediaKind::Video => self.stats_video_fwd += 1,
        }

        // Per-video-packet ingress trace (debug → only under ROOM_DEBUG).
        if kind == MediaKind::Video {
            tracing::debug!(
                pid = %source_pid,
                mid = %source_mid,
                ssrc = ?pkt.header.ssrc,
                seq = *pkt.seq_no,
                ts = pkt.header.timestamp,
                len = pkt.payload.len(),
                marker = pkt.header.marker,
                pt = ?pkt.header.payload_type,
                "video rtp in"
            );
        }

        // Top-K audio filter: drop audio from publishers not currently in
        // the speaker set. The set is recomputed on every tick; while it's
        // small enough (≤ K publishers) recompute keeps everyone in,
        // making this branch a no-op for typical 2-5 person calls.
        if kind == MediaKind::Audio && !in_top_audio {
            return;
        }

        let key = (source_pid, source_mid);

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

        let is_video = kind == MediaKind::Video;
        let mut wrote_any = false;
        for &(target_pid, target_mid) in &targets {
            let Some(session) = self.sessions.get_mut(&session_id) else {
                return;
            };
            let Some(target) = session.participants.get_mut(&target_pid) else {
                continue;
            };

            // Remap the publisher's codec to THIS subscriber's negotiated PT.
            // Each Rtc assigns PTs independently, so ingress PT != egress PT in
            // general — `match_params` finds the subscriber's param for the
            // same codec. Writing with the wrong PT makes str0m silently drop
            // the packet on send ("Media is missing PT"), i.e. black video.
            let egress_pt = match target.rtc.codec_config().match_params(source_pp) {
                Some(pp) => pp.pt(),
                None => {
                    // Subscriber never negotiated this codec — can't forward.
                    if is_video {
                        self.stats_video_no_pt += 1;
                    }
                    continue;
                }
            };

            // Egress sequence number: rebase the publisher's extended seq via
            // a signed per-stream offset, PRESERVING order and relative gaps.
            // str0m delivers RTP-mode packets in arrival order (de-RTX'd
            // resends land late), so a delivery-order counter would misorder
            // them and break multi-packet frame reassembly. Signed offset (not
            // `saturating_sub`) lets a reordered packet that extends BELOW the
            // first-arrived seq map below EGRESS_SEQ_START instead of clamping
            // onto it as a duplicate; on ingress SSRC change we re-anchor past
            // the highest seq sent. See `TrackOut::egress_seq_anchor`.
            let seq_no: str0m::rtp::SeqNo = {
                let Some(t) = target
                    .tracks_out
                    .iter_mut()
                    .find(|t| t.open_mid() == Some(target_mid))
                else {
                    continue;
                };
                let ingress = *pkt.seq_no as i64;
                let offset = match t.egress_seq_anchor {
                    Some((ssrc, offset)) if ssrc == pkt.header.ssrc => offset,
                    Some(_) => {
                        // Publisher restarted with a new SSRC on the same mid:
                        // str0m's seq extension starts over, the old offset is
                        // meaningless. Continue right after what we sent.
                        let offset = (t.egress_seq_high + 1) as i64 - ingress;
                        t.egress_seq_anchor = Some((pkt.header.ssrc, offset));
                        offset
                    }
                    None => {
                        let offset = EGRESS_SEQ_START as i64 - ingress;
                        t.egress_seq_anchor = Some((pkt.header.ssrc, offset));
                        offset
                    }
                };
                // max(1) is a belt-and-braces floor: START's headroom already
                // covers str0m's misorder window, so a hit means a broken
                // ingress extension — better one clamped packet than a huge
                // u64 wrap the receiver reads as astronomical loss.
                let egress = (ingress + offset).max(1) as u64;
                t.egress_seq_high = t.egress_seq_high.max(egress);
                egress.into()
            };

            // Header extensions do NOT pass through: MID/RID name m-lines of
            // the PUBLISHER's transport, TWCC seq numbers its feedback loop,
            // abs-send-time is its send clock — all transport-scoped. str0m
            // only overrides the MID ext until the subscriber's first RR acks
            // the SSRC (remote_acked_ssrc); after that a relayed publisher MID
            // ("1") goes on the wire verbatim, and Chrome's demuxer re-binds
            // the SSRC to whatever transceiver owns mid "1" on the subscriber
            // side — video freezes ~1s after camera start. Only end-to-end
            // media-level values are copied.
            let mut ext_vals = str0m::rtp::ExtensionValues::default();
            ext_vals.audio_level = pkt.header.ext_vals.audio_level;
            ext_vals.voice_activity = pkt.header.ext_vals.voice_activity;
            ext_vals.video_orientation = pkt.header.ext_vals.video_orientation;
            ext_vals.video_content_type = pkt.header.ext_vals.video_content_type;

            // Relay the publisher's RTP payload into the subscriber's send
            // stream: PT remapped (above), seq renumbered (contiguous). RTP
            // timestamp / marker pass through unchanged — single-layer, no
            // munging (that arrives with simulcast in step 2).
            {
                let mut da = target.rtc.direct_api();
                let Some(tx) = da.stream_tx_by_mid(target_mid, None) else {
                    if is_video {
                        self.stats_video_no_writer += 1;
                    }
                    continue;
                };
                // Send unpaced. str0m's default egress pacer (leaky bucket) holds
                // packets to a BWE-derived rate; with no BWE feeding it, it
                // barely releases anything → the receiver gets a handful of
                // packets (grey/frozen tile). An SFU relays media the publisher
                // ALREADY paced, so we forward as-arrived and let str0m emit
                // immediately. Idempotent flag; set every write is cheap.
                tx.set_unpaced(true);
                if let Err(e) = tx.write_rtp(
                    egress_pt,
                    seq_no,
                    pkt.header.timestamp,
                    pkt.timestamp,
                    pkt.header.marker,
                    ext_vals,
                    is_video, // nackable: video yes, audio no
                    pkt.payload.clone(),
                ) {
                    if is_video {
                        self.stats_video_write_err += 1;
                    }
                    tracing::trace!(pid = %target_pid, "rtp write skip: {e}");
                    continue;
                }
            }
            if is_video {
                wrote_any = true;
            }

            // Do NOT poll the target's Rtc here to push the packet out.
            // `poll_output` is a consuming queue, not a peek: an inline drain
            // loop that discards `Output::Event` eats the target's own pending
            // ingress `RtpPacket`s (in a 2-way call this silently dropped ~85%
            // of the reverse direction's video) and their `KeyframeRequest`s
            // (PLI never reached the publisher → permanent freeze after the
            // first loss). Instead, mark the target for a full-event-handler
            // poll at the end of this event-loop iteration; the stream is
            // unpaced, so that poll emits the Transmit immediately.
            self.pending_flush.insert((session_id, target_pid));

            // Egress diagnostics (debug → ROOM_DEBUG only). Per forwarded video
            // packet: which subscriber, the ingress→egress seq mapping, pt,
            // marker, ts.
            if is_video {
                tracing::debug!(
                    to = %&target_pid.to_string()[0..8],
                    in_seq = *pkt.seq_no,
                    eg_seq = *seq_no,
                    pt = ?egress_pt,
                    marker = pkt.header.marker,
                    ts = pkt.header.timestamp,
                    "video rtp out"
                );
            }
        }
        if wrote_any {
            self.stats_video_written += 1;
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
            std::mem::take(&mut self.pending_negotiation)
                .into_iter()
                .collect();

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
    fn parse_stun_local_ufrag_returns_recipient_segment_before_colon() {
        // Format on the wire is "{recipient}:{sender}" — recipient (us) is
        // first. Bug: previous version returned the sender's half and
        // silently broke ICE because every lookup missed.
        let pkt = build_stun_binding_with_username(b"my_local:remote_uf");
        assert_eq!(parse_stun_local_ufrag(&pkt), Some("my_local"));
    }

    #[test]
    fn parse_stun_local_ufrag_handles_padding_correctly() {
        // 3-byte username forces 1 byte of padding.
        let pkt = build_stun_binding_with_username(b"a:b");
        assert_eq!(parse_stun_local_ufrag(&pkt), Some("a"));
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
