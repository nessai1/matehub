import type {
  VideoClientOptions,
  VideoClientEvent,
  Participant,
  VideoDiagnostics,
  TrackInfo,
  TrackSource,
  TrackKind,
  ScreenShareProfile,
} from "./types";

type EventHandler = (event: VideoClientEvent) => void;

interface ScreenProfileConfig {
  width: number;
  height: number;
  fps: number;
  maxBitrate: number;
  contentHint: "motion" | "detail";
}

// Profile presets — see docs/video/screen-share.md §6.
const PROFILE_CONFIG: Record<ScreenShareProfile, ScreenProfileConfig> = {
  gaming: {
    width: 1920,
    height: 1080,
    fps: 60,
    maxBitrate: 6_000_000,
    contentHint: "motion",
  },
  standard: {
    width: 1280,
    height: 720,
    fps: 24,
    maxBitrate: 2_000_000,
    contentHint: "motion",
  },
  detail: {
    width: 2560,
    height: 1440,
    fps: 5,
    maxBitrate: 1_000_000,
    contentHint: "detail",
  },
};

// Camera capture constraints. The old defaults (640×480 hardcoded) made
// every camera tile a 480p upscale on a 720p/1080p layout — visibly
// blocky once the call grid stretched the source. 720p baseline plus an
// `ideal=1280/720` hint lets Chrome pick the camera's native sensor
// resolution if it can offer 1080p (Logitech BRIO, MacBook FaceTime
// 1080p, etc.); the `max` cap keeps a 4K webcam from blowing the
// upstream budget on a free-tier hub.
const CAMERA_CONSTRAINTS: MediaTrackConstraints = {
  width: { ideal: 1280, max: 1920 },
  height: { ideal: 720, max: 1080 },
  frameRate: { ideal: 30 },
};

// Encoder budget for a 720p talking head with motion. Chrome's default
// for a single-layer camera sender hovers around 1 Mbps — fine for static
// frames, blocky on hand gestures and lip motion. 2.5 Mbps clears the
// "Discord-quality" bar without overshooting what an ADSL uplink can
// swallow on the partner side. Use `maintain-framerate` so a CPU spike
// drops resolution before fps; a janky 720p call is worse UX than a
// brief 480p smoothness dip.
const CAMERA_TARGET_BITRATE = 2_500_000;
const CAMERA_TARGET_FPS = 30;

/**
 * MateHub Video SDK client.
 *
 * Manages WebSocket signaling, WebRTC peer connection, and media tracks
 * for connecting to a video/voice session.
 *
 * Usage:
 *   const client = new VideoClient({ serverUrl, sessionId, userId, token });
 *   client.on(event => { ... });
 *   await client.connect();
 *   await client.enableMic();
 *   await client.enableCamera();
 */
export class VideoClient {
  private opts: VideoClientOptions;
  private ws: WebSocket | null = null;
  private pc: RTCPeerConnection | null = null;
  private localStream: MediaStream | null = null;
  private participants = new Map<string, Participant>();
  private handlers: EventHandler[] = [];
  private participantId: string | null = null;
  private micEnabled = false;
  private camEnabled = false;
  private joined = false;
  private pendingCandidates: Array<{ candidate: string; sdpMid: string | null; sdpMLineIndex: number | null }> = [];
  private videoSender: RTCRtpSender | null = null;
  // Microphone-side companion of videoSender. Held so setMicDevice() can
  // hot-swap the audio track via replaceTrack() without renegotiating SDP.
  private audioSender: RTCRtpSender | null = null;
  // Currently-selected device IDs. Populated lazily — first getUserMedia
  // returns whatever the browser chose, and we read it off the resulting
  // track via getSettings().deviceId. setMicDevice / setCameraDevice update
  // these explicitly.
  private currentMicDeviceId: string | null = null;
  private currentCameraDeviceId: string | null = null;
  // Screen share state. Tracks are kept as object refs even after renegotiation
  // so reconnect flow (see screen-share doc §7.4) can silently restore them.
  private screenVideoTrack: MediaStreamTrack | null = null;
  private screenAudioTrack: MediaStreamTrack | null = null;
  private screenVideoSender: RTCRtpSender | null = null;
  private screenAudioSender: RTCRtpSender | null = null;
  /**
   * Stream_id → (source, kind) mapping rebuilt on every Offer's `tracks[]`.
   * Lets ontrack classify incoming tracks without parsing SDP. Separate from
   * this.participants since multiple streams alias one participant.
   */
  private streamMeta = new Map<string, { source: TrackSource; kind: TrackKind }>();
  /** Sequential processing queue -- prevents concurrent setRemoteDescription calls */
  private msgQueue: Promise<void> = Promise.resolve();

  // Audio level detection
  private audioContext: AudioContext | null = null;
  private analyserNodes = new Map<string, AnalyserNode>();
  private speakingInterval: ReturnType<typeof setInterval> | null = null;
  // Per-participant VAD state — tracks how many consecutive poll ticks the
  // participant has been below the "silence" threshold. Used to give a hang-over
  // window so normal speech pauses don't flap speaking off.
  private vadSilenceTicks = new Map<string, number>();

  constructor(opts: VideoClientOptions) {
    this.opts = opts;
  }

  /** Subscribe to events */
  on(handler: EventHandler) {
    this.handlers.push(handler);
    return () => {
      this.handlers = this.handlers.filter((h) => h !== handler);
    };
  }

  private emit(event: VideoClientEvent) {
    for (const handler of this.handlers) {
      try {
        handler(event);
      } catch (e) {
        console.error("Event handler error:", e);
      }
    }
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private log(msg: string, ...args: any[]) {
    console.log(`[VideoClient] ${msg}`, ...args);
    // Mirror to debug stream so the debug panel captures everything
    // the console shows, without consumers having to scrape the console.
    const data = args.length === 0 ? undefined : args.length === 1 ? args[0] : args;
    this.emit({ type: "debug", level: "info", msg, data });
  }

  private debug(level: "info" | "warn" | "error", msg: string, data?: unknown) {
    if (level === "warn") console.warn(`[VideoClient] ${msg}`, data);
    else if (level === "error") console.error(`[VideoClient] ${msg}`, data);
    else console.log(`[VideoClient] ${msg}`, data);
    this.emit({ type: "debug", level, msg, data });
  }

  /** Connect to the session: open WebSocket, create PeerConnection */
  async connect() {
    // Prevent double connect (React Strict Mode, HMR)
    if (this.ws || this.pc) {
      this.log("connect called but already connected, ignoring");
      return;
    }

    // serverUrl can be absolute or relative ("/api/video"); URL constructor
    // resolves both correctly against window.location.
    const base =
      typeof window !== "undefined"
        ? window.location.href
        : "http://localhost";
    const u = new URL(`${this.opts.serverUrl}/ws/${this.opts.sessionId}`, base);
    u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
    u.searchParams.set("user_id", this.opts.userId);
    u.searchParams.set("token", this.opts.token);
    if (this.opts.userUuid) u.searchParams.set("user_uuid", this.opts.userUuid);
    const wsUrl = u.toString();

    this.log("connect", { wsUrl });

    this.pc = new RTCPeerConnection({
      iceServers: this.opts.iceServers ?? [],
    });

    this.setupPeerConnection();
    this.connectWebSocket(wsUrl);
  }

  private connectWebSocket(url: string) {
    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      this.log("WS opened");
      this.createAndSendOffer();
    };

    this.ws.onmessage = (e) => {
      try {
        const msg = JSON.parse(e.data);
        // Chain onto queue so messages are processed sequentially.
        // Without this, concurrent setRemoteDescription calls race
        // when answer + offer arrive in the same tick.
        this.msgQueue = this.msgQueue.then(() => this.handleServerMessage(msg));
      } catch (err) {
        console.error("Failed to parse server message:", err);
      }
    };

    this.ws.onclose = () => {
      this.emit({ type: "disconnected", reason: "websocket closed" });
    };

    this.ws.onerror = () => {
      this.emit({ type: "error", message: "WebSocket error" });
    };
  }

  private setupPeerConnection() {
    const pc = this.pc!;

    // Trickle ICE: send candidates as they're discovered.
    // Buffer until joined (SFU needs Rtc to exist before accepting candidates).
    pc.onicecandidate = (e) => {
      if (!e.candidate) {
        this.log("ICE gathering complete");
        return;
      }
      const c = {
        candidate: e.candidate.candidate,
        sdpMid: e.candidate.sdpMid,
        sdpMLineIndex: e.candidate.sdpMLineIndex,
      };
      if (this.joined) {
        this.send({
          type: "ice_candidate",
          candidate: c.candidate,
          sdp_mid: c.sdpMid,
          sdp_mline_index: c.sdpMLineIndex,
        });
      } else {
        this.pendingCandidates.push(c);
      }
    };

    pc.oniceconnectionstatechange = () => {
      this.debug("info", "ICE connection state", { state: pc.iceConnectionState });
    };
    pc.onicegatheringstatechange = () => {
      this.debug("info", "ICE gathering state", { state: pc.iceGatheringState });
    };
    pc.onsignalingstatechange = () => {
      this.debug("info", "signaling state", { state: pc.signalingState });
    };

    // Handle remote tracks (from other participants via SFU)
    pc.ontrack = (e) => {
      this.log("ontrack!", {
        kind: e.track.kind,
        trackId: e.track.id,
        streamCount: e.streams.length,
        streamIds: e.streams.map((s) => s.id),
        joined: this.joined,
      });

      // Ignore tracks that arrive before we're joined (from initial SDP answer).
      // Real remote tracks come via renegotiation offers AFTER join.
      if (!this.joined) {
        this.log("ignoring ontrack before join completed");
        return;
      }

      // Ignore tracks without an associated stream
      if (e.streams.length === 0) {
        this.log("ignoring ontrack with no streams");
        return;
      }

      const stream = e.streams[0];
      const streamId = stream.id;

      // Match stream to participant.
      // Stream_id mapping is registered when we receive "offer" with tracks array.
      // The mapping adds participant under stream_id key in this.participants.
      const participant = this.participants.get(streamId);

      if (!participant) {
        // No mapping — this used to spawn a "remote" placeholder, which
        // hid real desync bugs (msid sanitization, out-of-order offer/ontrack)
        // behind ghost tiles in the UI. Now we drop the track and surface
        // the mismatch in the debug panel.
        this.debug("warn", "ontrack without participant mapping — dropping", {
          streamId,
          trackKind: e.track.kind,
          knownStreams: Array.from(this.participants.keys()),
        });
        return;
      }

      // Resolve source+kind from the streamMeta table we built off the
      // last `offer.tracks[]` payload. Without meta we can still render
      // the track but we don't know whether it's camera or screen.
      const meta = this.streamMeta.get(streamId);
      const source: TrackSource = meta?.source ?? "camera";
      const kind: TrackKind = (meta?.kind ?? e.track.kind) as TrackKind;

      if (kind === "audio" && source === "camera") {
        participant.audioTrack = e.track;
        this.setupAudioLevelDetection(participant.participantId, e.track);
      } else if (kind === "video" && source === "camera") {
        participant.videoTrack = e.track;
        // Browser mute/unmute events are unreliable for camera state
        // (RTCP triggers onunmute even with replaceTrack(null)).
        // We use explicit signaling instead -- see "participant_muted" handler.
        e.track.onmute = () => {
          this.log("remote video track muted (browser event, ignoring)", {
            participantId: participant!.participantId,
          });
        };
        e.track.onunmute = () => {
          this.log("remote video track unmuted (browser event, ignoring)", {
            participantId: participant!.participantId,
          });
        };
      } else if (kind === "video" && source === "screen") {
        participant.screenVideoTrack = e.track;
        this.emit({
          type: "screen_share_started",
          participantId: participant.participantId,
        });
        e.track.onended = () => {
          participant.screenVideoTrack = null;
          this.emit({
            type: "screen_share_stopped",
            participantId: participant!.participantId,
          });
        };
      } else if (kind === "audio" && source === "screen") {
        participant.screenAudioTrack = e.track;
      }

      this.emit({
        type: "track_added",
        participantId: participant.participantId,
        track: e.track,
        stream,
        source,
        kind,
      });
    };

    pc.onconnectionstatechange = () => {
      this.debug("info", "PC connection state", { state: pc.connectionState });
      // "disconnected" in WebRTC is a transient state — ICE restart / route
      // recovery can restore the connection. Only treat failed/closed as fatal,
      // otherwise a brief network hiccup flips the UI back to "Connecting…".
      if (pc.connectionState === "failed" || pc.connectionState === "closed") {
        this.emit({ type: "disconnected", reason: `peer connection ${pc.connectionState}` });
      }
    };
  }

  private async createAndSendOffer() {
    const pc = this.pc;
    if (!pc) {
      this.debug("warn", "createAndSendOffer: PC gone before start");
      return;
    }
    // The user may have hit Leave while getUserMedia's permission prompt was
    // open — in that case disconnect() has already closed `pc`. Every `await`
    // below is a chance for that to happen; check the state after each one
    // and bail cleanly instead of calling methods on a closed PC (which
    // throws InvalidStateError and drops a Next.js error overlay, effectively
    // locking the UI).
    //
    // TS's RTCSignalingState type excludes "closed" after the first narrowing
    // check since it doesn't know the state mutates across awaits; the cast
    // is deliberate, not a code smell.
    const isClosed = () => (pc.signalingState as string) === "closed";

    if (isClosed()) {
      this.debug("warn", "createAndSendOffer: PC closed before start");
      return;
    }

    // Get user media BEFORE creating offer so tracks are in the SDP.
    // This ensures str0m sees actual sending tracks, not empty sendrecv transceivers.
    // Tracks are added but MUTED by default -- user enables them explicitly.
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: true,
        video: { width: 640, height: 480 },
      });
      if (isClosed()) {
        stream.getTracks().forEach((t) => t.stop());
        return;
      }
      this.localStream = stream;

      for (const track of stream.getTracks()) {
        track.enabled = false; // muted by default
        const sender = pc.addTrack(track, stream);
        const settingsDeviceId = track.getSettings().deviceId ?? null;
        if (track.kind === "video") {
          this.videoSender = sender;
          this.currentCameraDeviceId = settingsDeviceId;
          // track.enabled=false still sends black-frame RTP, which triggers
          // onunmute on the receiver -> grey tile instead of avatar.
          // replaceTrack(null) stops RTP entirely -> receiver track stays muted.
          sender.replaceTrack(null);
        } else if (track.kind === "audio") {
          this.audioSender = sender;
          this.currentMicDeviceId = settingsDeviceId;
        }
        this.log("added local track to PC (muted)", { kind: track.kind, id: track.id });
      }

      this.micEnabled = false;
      this.camEnabled = false;
    } catch (e) {
      this.log("getUserMedia failed, falling back to recvonly", e);
      // Fallback: receive-only if no camera/mic available. Same guard —
      // disconnect() during the permission prompt leaves pc closed here.
      if (isClosed()) return;
      pc.addTransceiver("audio", { direction: "recvonly" });
      pc.addTransceiver("video", { direction: "recvonly" });
    }

    if (isClosed()) return;
    const offer = await pc.createOffer();
    if (isClosed()) return;
    await pc.setLocalDescription(offer);
    if (isClosed()) return;

    // Trickle ICE: send offer immediately, candidates will follow via ice_candidate messages.
    // No waiting for gathering -- shaves seconds off connect time.
    this.log("sending join (trickle ICE, candidates sent separately)");

    this.send({
      type: "join",
      sdp_offer: offer.sdp,
    });
  }

  private async handleServerMessage(msg: Record<string, unknown>) {
    this.log("server msg:", msg.type, msg);
    // Server messages are processed through msgQueue — if disconnect() fires
    // between a message arriving and us getting scheduled, pc is gone.
    // Rather than throwing on every SDP op, just drop the message.
    // (See createAndSendOffer for the same cast explanation.)
    if (!this.pc || (this.pc.signalingState as string) === "closed") {
      this.debug("warn", "handleServerMessage: PC closed, ignoring", {
        type: msg.type,
      });
      return;
    }
    switch (msg.type) {
      case "answer": {
        await this.pc!.setRemoteDescription({
          type: "answer",
          sdp: msg.sdp_answer as string,
        });
        this.participantId = msg.participant_id as string;
        this.joined = true;

        // Flush buffered ICE candidates (collected before answer arrived)
        this.log(`flushing ${this.pendingCandidates.length} buffered ICE candidates`);
        for (const c of this.pendingCandidates) {
          this.send({
            type: "ice_candidate",
            candidate: c.candidate,
            sdp_mid: c.sdpMid,
            sdp_mline_index: c.sdpMLineIndex,
          });
        }
        this.pendingCandidates = [];

        this.emit({ type: "connected", participantId: this.participantId });
        break;
      }

      case "offer": {
        // Register stream_id -> participant + source/kind mapping from SFU
        const tracks = msg.tracks as
          | Array<{
              stream_id: string;
              participant_id: string;
              user_id: string;
              source?: string;
              kind?: string;
            }>
          | undefined;

        if (tracks) {
          for (const t of tracks) {
            this.log("registering stream mapping", t);
            // Store mapping so ontrack can find participant by stream_id
            if (!this.participants.has(t.stream_id)) {
              // Check if participant exists under participant_id
              const existing = this.participants.get(t.participant_id);
              if (existing) {
                // Also index by stream_id for ontrack lookup
                this.participants.set(t.stream_id, existing);
              }
            }
            // Remember source+kind so ontrack can route screen vs camera.
            if (t.source && t.kind) {
              this.streamMeta.set(t.stream_id, {
                source: t.source as TrackSource,
                kind: t.kind as TrackKind,
              });
            }
          }
        }

        // SFU renegotiation (new tracks available)
        await this.pc!.setRemoteDescription({
          type: "offer",
          sdp: msg.sdp_offer as string,
        });
        if ((this.pc!.signalingState as string) === "closed") break;
        const answer = await this.pc!.createAnswer();
        if ((this.pc!.signalingState as string) === "closed") break;
        await this.pc!.setLocalDescription(answer);
        if ((this.pc!.signalingState as string) === "closed") break;
        this.send({
          type: "answer",
          sdp_answer: answer.sdp,
        });
        break;
      }

      case "ice_candidate": {
        try {
          await this.pc!.addIceCandidate({
            candidate: msg.candidate as string,
            sdpMid: msg.sdp_mid as string | null,
            sdpMLineIndex: msg.sdp_mline_index as number | null,
          });
        } catch (e) {
          // Late ICE candidate after PC closed — non-fatal.
          this.debug("warn", "addIceCandidate failed", { error: String(e) });
        }
        break;
      }

      case "participant_joined": {
        const p: Participant = {
          participantId: msg.participant_id as string,
          userId: msg.user_id as string,
          audioTrack: null,
          videoTrack: null,
          screenVideoTrack: null,
          screenAudioTrack: null,
          isSpeaking: false,
          isMicMuted: true,
          // Server replays "ParticipantDeafened" right after this for
          // late joiners whose deafen state is true; default-false is
          // correct here.
          isDeafened: false,
          stream: new MediaStream(),
        };
        this.participants.set(p.participantId, p);
        this.emit({ type: "participant_joined", participant: p });
        break;
      }

      case "participant_left": {
        const pid = msg.participant_id as string;
        const uid = msg.user_id as string;
        this.participants.delete(pid);
        this.emit({ type: "participant_left", participantId: pid, userId: uid });
        break;
      }

      case "participant_muted": {
        const pid = msg.participant_id as string;
        const kind = msg.kind as string;
        const muted = msg.muted as boolean;
        this.log("participant_muted (signaling)", { pid, kind, muted });
        // Update our local Participant view too. Without this the SDK fan-outs
        // the event but the next time anything reads `participant.isMicMuted`
        // (diagnostics, the "mic muted" badge in the UI) it sees the stale
        // initial `true`. Audio plays fine over WebRTC regardless of this
        // flag — but the badge stays stuck and that's how users get confused.
        const participant = this.participants.get(pid);
        if (participant && kind === "audio") {
          participant.isMicMuted = muted;
        }
        this.emit({
          type: "track_muted",
          participantId: pid,
          trackKind: kind,
          muted,
        });
        break;
      }

      case "participant_deafened": {
        const pid = msg.participant_id as string;
        const deafened = msg.deafened as boolean;
        this.log("participant_deafened (signaling)", { pid, deafened });
        // Mirror onto the local Participant view, same rationale as the
        // mute mirror just above (stale flag → stale badge).
        const participant = this.participants.get(pid);
        if (participant) {
          participant.isDeafened = deafened;
        }
        this.emit({
          type: "deafen_changed",
          participantId: pid,
          deafened,
        });
        break;
      }

      case "error": {
        this.emit({ type: "error", message: msg.message as string });
        break;
      }

      case "force_disconnected": {
        // Server is kicking us — typically because the same user joined
        // this session from another tab. Distinct event from generic
        // `disconnected` so the call UI can leave the call + show a
        // friendly toast instead of treating this as a network blip.
        const reason = (msg.reason as string) || "force_disconnected";
        this.log("force disconnected by server", { reason });
        this.emit({ type: "force_disconnected", reason });
        // Tear down the WS — server has already dropped its Rtc, no point
        // keeping the socket around.
        try {
          this.ws?.close(1000, "force_disconnected");
        } catch {
          /* ignore */
        }
        break;
      }
    }
  }

  private send(msg: Record<string, unknown>) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  // ── Media controls ─────────────────────────────

  /** Enable microphone: first call acquires device, subsequent calls re-enable track */
  async enableMic() {
    this.log("enableMic", { micEnabled: this.micEnabled, hasLocalStream: !!this.localStream });
    if (this.micEnabled) return;

    const existingTrack = this.localStream?.getAudioTracks()[0];
    this.log("enableMic existingTrack", { exists: !!existingTrack, enabled: existingTrack?.enabled, readyState: existingTrack?.readyState });

    let needsRenegotiation = false;

    if (existingTrack && existingTrack.readyState === "live") {
      existingTrack.enabled = true;
      this.log("enableMic re-enabled existing track");
    } else {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const track = stream.getAudioTracks()[0];
      this.log("enableMic acquired new track", { trackId: track?.id, pcState: this.pc?.signalingState });
      if (track && this.pc) {
        if (!this.localStream) this.localStream = new MediaStream();
        this.localStream.addTrack(track);
        if (this.audioSender) {
          // Existing sender → just swap the track. No SDP touch needed.
          await this.audioSender.replaceTrack(track);
        } else {
          // First-time send: addTrack creates a new m-line. Without an offer/
          // answer round-trip the SFU will never see this track and the peer
          // hears silence even though the local sender shows the track as live.
          this.audioSender = this.pc.addTrack(track, this.localStream);
          needsRenegotiation = true;
        }
        this.currentMicDeviceId = track.getSettings().deviceId ?? null;
        this.log("enableMic added track to PC, senders:", this.pc.getSenders().length);
      }
    }
    this.micEnabled = true;
    this.send({ type: "mute_changed", kind: "audio", muted: false });
    if (needsRenegotiation) {
      await this.renegotiate("enableMic");
    }
  }

  /** Disable microphone (mutes track, keeps it in PeerConnection) */
  disableMic() {
    const track = this.localStream?.getAudioTracks()[0];
    this.log("disableMic", { trackExists: !!track, enabled: track?.enabled, readyState: track?.readyState });
    if (track) track.enabled = false;
    this.micEnabled = false;
    this.send({ type: "mute_changed", kind: "audio", muted: true });
  }

  /** Enable camera: first call acquires device, subsequent calls re-enable track */
  async enableCamera() {
    this.log("enableCamera", { camEnabled: this.camEnabled, hasLocalStream: !!this.localStream });
    if (this.camEnabled) return;

    const existingTrack = this.localStream?.getVideoTracks()[0];
    this.log("enableCamera existingTrack", { exists: !!existingTrack, enabled: existingTrack?.enabled, readyState: existingTrack?.readyState });

    let needsRenegotiation = false;

    if (existingTrack && existingTrack.readyState === "live") {
      existingTrack.enabled = true;
      // Re-apply CAMERA_CONSTRAINTS on the existing track in case it was
      // acquired before the resolution bump (or by a setCameraDevice call
      // that pre-dated this fix). applyConstraints is best-effort: the
      // browser can refuse and keep the current resolution, which is fine.
      try {
        await existingTrack.applyConstraints(CAMERA_CONSTRAINTS);
      } catch (e) {
        this.debug("warn", "enableCamera applyConstraints failed", { error: String(e) });
      }
      await this.videoSender?.replaceTrack(existingTrack);
      this.log("enableCamera re-enabled existing track via replaceTrack");
    } else {
      const stream = await navigator.mediaDevices.getUserMedia({
        video: CAMERA_CONSTRAINTS,
      });
      const track = stream.getVideoTracks()[0];
      this.log("enableCamera acquired new track", { trackId: track?.id });
      if (track && this.pc) {
        if (!this.localStream) this.localStream = new MediaStream();
        this.localStream.addTrack(track);
        if (this.videoSender) {
          await this.videoSender.replaceTrack(track);
        } else {
          // Same SDP-renegotiation rationale as enableMic — first-time send
          // needs an offer/answer round-trip or the SFU never wires it up.
          this.videoSender = this.pc.addTrack(track, this.localStream);
          needsRenegotiation = true;
        }
        this.currentCameraDeviceId = track.getSettings().deviceId ?? null;
        this.log("enableCamera track on PC, senders:", this.pc.getSenders().length);
      }
    }

    // Apply encoder caps on whatever sender is now live. Done after the
    // sender exists for both the renegotiation path (just added) and the
    // replaceTrack path (already existed). setParameters is idempotent —
    // repeated calls just re-affirm the same encoding shape.
    if (this.videoSender) {
      await this.applyCameraEncoderParams(this.videoSender);
    }

    this.camEnabled = true;
    this.send({ type: "mute_changed", kind: "video", muted: false });
    if (needsRenegotiation) {
      await this.renegotiate("enableCamera");
    }
  }

  /** Pin the camera sender's encoder to the project's target bitrate /
   *  framerate / degradation policy. See CAMERA_TARGET_BITRATE doc.
   *
   *  Single-layer only — bails with a debug-warn if it finds ≥2
   *  encodings. The naive "for-each encoding apply same cap" loop here
   *  before the guard would have starved the lowest simulcast layer
   *  (intended ~300–500 kbps) by giving it the 2.5 Mbps cap of the
   *  highest, which then fights the highest layer for upstream during
   *  BWE adaptation. When camera-simulcast lands it needs to carry its
   *  own per-rid bitrate table; that's a feature change, not a one-line
   *  loop. Until then this method only handles the single-encoding case
   *  and refuses to silently mis-tune anything else. */
  private async applyCameraEncoderParams(sender: RTCRtpSender) {
    try {
      const params = sender.getParameters();
      if (!params.encodings || params.encodings.length === 0) {
        params.encodings = [{}];
      }
      if (params.encodings.length !== 1) {
        this.debug("warn", "applyCameraEncoderParams: skipping for simulcast sender", {
          encodings: params.encodings.length,
          hint: "camera-simulcast must provide its own per-rid bitrate plan",
        });
        // Still set degradationPreference — it's a sender-wide knob,
        // not per-encoding, so applying it is safe regardless of layer
        // count and keeps the "maintain-framerate" intent intact.
        params.degradationPreference = "maintain-framerate";
        await sender.setParameters(params);
        return;
      }
      params.encodings[0].maxBitrate = CAMERA_TARGET_BITRATE;
      params.encodings[0].maxFramerate = CAMERA_TARGET_FPS;
      params.degradationPreference = "maintain-framerate";
      await sender.setParameters(params);
    } catch (e) {
      this.debug("warn", "applyCameraEncoderParams failed", { error: String(e) });
    }
  }

  /** Disable camera (mutes track, keeps it in PeerConnection) */
  disableCamera() {
    const track = this.localStream?.getVideoTracks()[0];
    this.log("disableCamera", { trackExists: !!track, enabled: track?.enabled, readyState: track?.readyState });
    if (track) track.enabled = false;
    // Stop RTP entirely so receiver shows avatar, not black tile
    this.videoSender?.replaceTrack(null);
    this.camEnabled = false;
    this.send({ type: "mute_changed", kind: "video", muted: true });
  }

  // ── Device selection ───────────────────────────
  //
  // The microphone and camera sit behind a single RTCRtpSender each
  // (this.audioSender / this.videoSender). Switching device == swapping the
  // MediaStreamTrack via sender.replaceTrack(); SSRC, m-line and SDP all
  // stay the same, so the SFU sees an uninterrupted stream and no
  // renegotiation is required. Same trick the screen-share path leans on
  // for resolution swaps.
  //
  // Caller responsibility: catch the AbortError / NotFoundError / etc. that
  // getUserMedia throws when the requested device is gone or permission was
  // revoked, and re-prompt the user.

  /** Enumerate available media input devices. Labels are populated only
   *  once permission has been granted at least once for that kind. */
  async listDevices(): Promise<{
    audioInputs: MediaDeviceInfo[];
    videoInputs: MediaDeviceInfo[];
  }> {
    const all = await navigator.mediaDevices.enumerateDevices();
    return {
      audioInputs: all.filter((d) => d.kind === "audioinput"),
      videoInputs: all.filter((d) => d.kind === "videoinput"),
    };
  }

  /** Currently active microphone deviceId, or null if not yet acquired. */
  getCurrentMicDeviceId(): string | null {
    return this.currentMicDeviceId;
  }

  /** Currently active camera deviceId, or null if not yet acquired. */
  getCurrentCameraDeviceId(): string | null {
    return this.currentCameraDeviceId;
  }

  /** Switch the microphone to a different input device. The new track
   *  inherits the current mute state (so silently muted callers stay
   *  silent). No SDP renegotiation. */
  async setMicDevice(deviceId: string) {
    if (!this.pc) {
      this.debug("warn", "setMicDevice: not connected");
      return;
    }
    if (this.currentMicDeviceId === deviceId) {
      this.log("setMicDevice: already on this device", { deviceId });
      return;
    }

    const stream = await navigator.mediaDevices.getUserMedia({
      audio: { deviceId: { exact: deviceId } },
    });
    const newTrack = stream.getAudioTracks()[0];
    if (!newTrack) {
      throw new Error("setMicDevice: getUserMedia returned no audio track");
    }
    // Inherit mute state — caller didn't ask to unmute, only to switch source.
    newTrack.enabled = this.micEnabled;

    // Drop the old track from localStream, stop it (releases the device),
    // splice in the new one.
    if (!this.localStream) this.localStream = new MediaStream();
    for (const t of this.localStream.getAudioTracks()) {
      this.localStream.removeTrack(t);
      t.stop();
    }
    this.localStream.addTrack(newTrack);

    if (this.audioSender) {
      await this.audioSender.replaceTrack(newTrack);
    } else {
      this.audioSender = this.pc.addTrack(newTrack, this.localStream);
    }
    this.currentMicDeviceId = newTrack.getSettings().deviceId ?? deviceId;
    this.log("setMicDevice done", { deviceId: this.currentMicDeviceId });
  }

  /** Switch the camera to a different input device. Honors current camera
   *  on/off — if the camera is off, the new track is parked in localStream
   *  and the sender stays at null until the user re-enables the camera. */
  async setCameraDevice(deviceId: string) {
    if (!this.pc) {
      this.debug("warn", "setCameraDevice: not connected");
      return;
    }
    if (this.currentCameraDeviceId === deviceId) {
      this.log("setCameraDevice: already on this device", { deviceId });
      return;
    }

    const stream = await navigator.mediaDevices.getUserMedia({
      // `deviceId: exact` pins the source; the rest mirror CAMERA_CONSTRAINTS
      // so a mid-call switch doesn't quietly downgrade the partner from
      // 720p back to 480p.
      video: {
        deviceId: { exact: deviceId },
        ...CAMERA_CONSTRAINTS,
      },
    });
    const newTrack = stream.getVideoTracks()[0];
    if (!newTrack) {
      throw new Error("setCameraDevice: getUserMedia returned no video track");
    }
    // If camera was disabled, keep the new track muted-and-parked too. Black
    // RTP would otherwise wake the receiver up to a grey tile.
    newTrack.enabled = this.camEnabled;

    if (!this.localStream) this.localStream = new MediaStream();
    for (const t of this.localStream.getVideoTracks()) {
      this.localStream.removeTrack(t);
      t.stop();
    }
    this.localStream.addTrack(newTrack);

    if (this.videoSender) {
      // Sender exists from the initial offer. If cam is on, stream new
      // frames; if off, keep RTP closed but hold the track for re-enable.
      await this.videoSender.replaceTrack(this.camEnabled ? newTrack : null);
    } else {
      this.videoSender = this.pc.addTrack(newTrack, this.localStream);
      if (!this.camEnabled) await this.videoSender.replaceTrack(null);
    }
    // Re-apply encoder caps. A device swap re-creates the underlying
    // codec context in Chrome and any previous `setParameters` is lost —
    // without this the new device starts at Chrome's default 1 Mbps.
    if (this.videoSender) {
      await this.applyCameraEncoderParams(this.videoSender);
    }
    this.currentCameraDeviceId = newTrack.getSettings().deviceId ?? deviceId;
    this.log("setCameraDevice done", {
      deviceId: this.currentCameraDeviceId,
      camEnabled: this.camEnabled,
    });
  }

  /** Toggle mic on/off */
  async toggleMic() {
    this.log("toggleMic", { micEnabled: this.micEnabled });
    if (this.micEnabled) {
      this.disableMic();
    } else {
      await this.enableMic();
    }
    this.log("toggleMic result", { micEnabled: this.micEnabled });
    return this.micEnabled;
  }

  /** Toggle camera on/off */
  async toggleCamera() {
    this.log("toggleCamera", { camEnabled: this.camEnabled });
    if (this.camEnabled) {
      this.disableCamera();
    } else {
      await this.enableCamera();
    }
    this.log("toggleCamera result", { camEnabled: this.camEnabled });
    return this.camEnabled;
  }

  /** Broadcast "I'm deafened" / "I'm undeafened" through the SFU so
   *  other participants render a headphone-off icon on this tile.
   *
   *  Deafen is purely a UI signal — the actual audio suppression happens
   *  on the deafened client itself (gain=0 on every remote track). The
   *  SFU still forwards audio to the deafened client; we just promise
   *  not to play it. Untreated locally this would burn upstream
   *  bandwidth on a stream nobody hears, but a smarter "tell SFU to
   *  stop sending me audio when deafened" optimisation lands in its
   *  own ticket — for the visual MVP, broadcast-only is enough. */
  setDeafened(deafened: boolean) {
    this.log("setDeafened", { deafened });
    this.send({ type: "deafen_changed", deafened });
  }

  get isMicEnabled() {
    return this.micEnabled;
  }

  get isCamEnabled() {
    return this.camEnabled;
  }

  /**
   * Start sharing screen with the given quality profile.
   *
   * Flow (matches docs/video/screen-share.md §5):
   * 1. `getDisplayMedia` — browser prompts user to pick a source.
   * 2. `addTransceiver` with 2-layer simulcast sendEncodings.
   * 3. `setParameters` tunes maxBitrate/framerate to the profile.
   * 4. `contentHint` tells the encoder "motion" vs "detail".
   * 5. Send `publish_track` so the SFU tags the next MediaAdded as screen.
   * 6. Send audio track the same way if the OS let us capture it.
   * 7. Create+send client-initiated Offer; SFU answers via `Answer` handler.
   */
  async publishScreen(profile: ScreenShareProfile = "gaming") {
    if (!this.pc) {
      throw new Error("publishScreen: not connected");
    }
    if (this.screenVideoTrack) {
      this.log("publishScreen: already sharing, ignoring");
      return;
    }

    const profileConfig = PROFILE_CONFIG[profile];

    let displayStream: MediaStream;
    try {
      displayStream = await navigator.mediaDevices.getDisplayMedia({
        video: {
          frameRate: { ideal: profileConfig.fps, max: profileConfig.fps },
          width: { ideal: profileConfig.width, max: profileConfig.width },
          height: { ideal: profileConfig.height, max: profileConfig.height },
        },
        // System/tab audio — null on macOS full-screen, that's fine.
        audio: true,
      });
    } catch (e) {
      // User cancelled the picker or permission denied.
      this.debug("warn", "getDisplayMedia failed", { error: String(e) });
      throw e;
    }

    const videoTrack = displayStream.getVideoTracks()[0];
    const audioTrack = displayStream.getAudioTracks()[0] ?? null;
    if (!videoTrack) {
      throw new Error("publishScreen: no video track from getDisplayMedia");
    }

    videoTrack.contentHint = profileConfig.contentHint;
    this.screenVideoTrack = videoTrack;
    this.screenAudioTrack = audioTrack;

    // Tell the SFU these tracks are `source: screen` BEFORE the SDP offer.
    // WS preserves order within one socket — the SFU queues the hint and
    // pops it on MediaAdded.
    this.send({
      type: "publish_track",
      source: "screen",
      kind: "video",
      track_id: videoTrack.id,
    });
    if (audioTrack) {
      this.send({
        type: "publish_track",
        source: "screen",
        kind: "audio",
        track_id: audioTrack.id,
      });
    }

    // Add the video track with 2-layer simulcast.
    const videoTransceiver = this.pc.addTransceiver(videoTrack, {
      direction: "sendonly",
      streams: [displayStream],
      sendEncodings: [
        {
          rid: "h",
          maxBitrate: profileConfig.maxBitrate,
          maxFramerate: profileConfig.fps,
        },
        {
          rid: "l",
          maxBitrate: 400_000,
          maxFramerate: 15,
          scaleResolutionDownBy: Math.max(1, profileConfig.width / 720),
        },
      ],
    });
    this.screenVideoSender = videoTransceiver.sender;

    // Tell Chrome how to react when the encoder runs out of CPU or the
    // BWE shrinks: motion profiles (gaming, standard) prefer to drop
    // resolution over framerate — a 720p 30fps gameplay demo beats a
    // sharp 1080p 8fps slideshow. Detail flips that — a doc reviewer
    // wants legible text more than smooth animation. Default is
    // "balanced" which drops fps first and hurts the gameplay case.
    try {
      const params = this.screenVideoSender.getParameters();
      params.degradationPreference =
        profileConfig.contentHint === "detail"
          ? "maintain-resolution"
          : "maintain-framerate";
      await this.screenVideoSender.setParameters(params);
    } catch (e) {
      this.debug("warn", "publishScreen setParameters failed", { error: String(e) });
    }

    if (audioTrack) {
      const audioTransceiver = this.pc.addTransceiver(audioTrack, {
        direction: "sendonly",
        streams: [displayStream],
      });
      this.screenAudioSender = audioTransceiver.sender;
    }

    // Auto-unpublish when the user hits "Stop sharing" in Chrome's bar,
    // unplugs the monitor, or revokes permission.
    videoTrack.onended = () => {
      this.log("screen video track ended, unpublishing");
      void this.unpublishScreen();
    };
    if (audioTrack) {
      audioTrack.onended = () => {
        // Audio-only end (rare) — leave video running, just null out audio.
        this.log("screen audio track ended");
        this.screenAudioTrack = null;
      };
    }

    // Client-initiated renegotiation.
    const offer = await this.pc.createOffer();
    await this.pc.setLocalDescription(offer);
    this.send({ type: "offer", sdp_offer: offer.sdp });

    this.debug("info", "publishScreen completed", {
      profile,
      hasAudio: !!audioTrack,
    });
  }

  /** Stop sharing: remove senders, stop tracks, renegotiate. */
  async unpublishScreen() {
    if (!this.pc || !this.screenVideoTrack) return;

    if (this.screenVideoSender) {
      try {
        this.pc.removeTrack(this.screenVideoSender);
      } catch (e) {
        this.debug("warn", "removeTrack(screenVideo) failed", { error: String(e) });
      }
      this.screenVideoSender = null;
    }
    if (this.screenAudioSender) {
      try {
        this.pc.removeTrack(this.screenAudioSender);
      } catch (e) {
        this.debug("warn", "removeTrack(screenAudio) failed", { error: String(e) });
      }
      this.screenAudioSender = null;
    }

    this.screenVideoTrack.stop();
    this.screenAudioTrack?.stop();
    this.screenVideoTrack = null;
    this.screenAudioTrack = null;

    const offer = await this.pc.createOffer();
    await this.pc.setLocalDescription(offer);
    this.send({ type: "offer", sdp_offer: offer.sdp });

    this.debug("info", "unpublishScreen completed");
  }

  get isScreenSharing() {
    return this.screenVideoTrack !== null;
  }

  /** Local screen-video track for self-preview (null when not sharing). */
  getScreenVideoTrack() {
    return this.screenVideoTrack;
  }

  /** Local screen-audio track for self-preview. Usually null on macOS. */
  getScreenAudioTrack() {
    return this.screenAudioTrack;
  }

  /**
   * Client-initiated SDP renegotiation. Used after addTrack() creates a new
   * sender (first enableMic / enableCamera) — until offer/answer completes,
   * the SFU has no m-line for that track and the peer hears/sees nothing.
   * Screen-share path open-codes the same three calls; one day they could
   * collapse into this helper too.
   */
  private async renegotiate(reason: string) {
    if (!this.pc || !this.ws) return;
    try {
      const offer = await this.pc.createOffer();
      await this.pc.setLocalDescription(offer);
      this.send({ type: "offer", sdp_offer: offer.sdp });
      this.log("renegotiate sent offer", { reason });
    } catch (e) {
      this.debug("warn", "renegotiate failed", { reason, error: String(e) });
    }
  }

  // ── Speaking detection ─────────────────────────

  private setupAudioLevelDetection(participantId: string, track: MediaStreamTrack) {
    if (!this.audioContext) {
      this.audioContext = new AudioContext();
    }

    const source = this.audioContext.createMediaStreamSource(new MediaStream([track]));
    const analyser = this.audioContext.createAnalyser();
    analyser.fftSize = 512;
    analyser.smoothingTimeConstant = 0.3; // less smoothing = faster response
    source.connect(analyser);
    this.analyserNodes.set(participantId, analyser);

    if (!this.speakingInterval) {
      // 60ms poll -- fast response, low CPU (just reading a buffer)
      this.speakingInterval = setInterval(() => this.checkSpeakingLevels(), 60);
    }
  }

  private checkSpeakingLevels() {
    // Voice energy concentrates in 85-1000 Hz.
    // With fftSize=512 at 48kHz sample rate, each bin = ~94 Hz.
    // Bins 1-11 cover roughly 94-1034 Hz (the voice fundamental range).
    const VOICE_BIN_START = 1;
    const VOICE_BIN_END = 12;
    // Hysteresis: require louder level to START speaking than to STAY speaking.
    // Without the gap the state flapped 50 times/sec around a single threshold
    // (especially with two browsers on one machine picking up each other's
    // echo). Numbers empirical for Opus decoded at ~-30dBFS speech.
    const THRESHOLD_ON = 22;
    const THRESHOLD_OFF = 12;
    // After level drops below OFF, keep showing "speaking" for this many ticks
    // so natural pauses between words don't gap out the indicator. 8 * 60ms ≈ 480ms.
    const MAX_SILENCE_TICKS = 8;
    const data = new Uint8Array(256); // fftSize/2

    for (const [pid, analyser] of this.analyserNodes) {
      analyser.getByteFrequencyData(data);

      // Average only the voice-frequency bins, not the whole spectrum
      let sum = 0;
      for (let i = VOICE_BIN_START; i < VOICE_BIN_END; i++) {
        sum += data[i];
      }
      const avg = sum / (VOICE_BIN_END - VOICE_BIN_START);

      const participant = this.participants.get(pid);
      if (!participant) continue;

      const wasSpeaking = participant.isSpeaking;
      let silenceTicks = this.vadSilenceTicks.get(pid) ?? 0;
      let newSpeaking = wasSpeaking;

      if (wasSpeaking) {
        if (avg < THRESHOLD_OFF) {
          silenceTicks++;
          if (silenceTicks >= MAX_SILENCE_TICKS) {
            newSpeaking = false;
            silenceTicks = 0;
          }
        } else {
          silenceTicks = 0;
        }
      } else if (avg > THRESHOLD_ON) {
        newSpeaking = true;
        silenceTicks = 0;
      }

      this.vadSilenceTicks.set(pid, silenceTicks);

      if (newSpeaking !== wasSpeaking) {
        participant.isSpeaking = newSpeaking;
        this.emit({
          type: "speaking_changed",
          participantId: pid,
          speaking: newSpeaking,
        });
      }
    }
  }

  // ── Getters ────────────────────────────────────

  /** Get all remote participants */
  getParticipants(): Participant[] {
    return Array.from(this.participants.values());
  }

  /** Get local media stream */
  getLocalStream(): MediaStream | null {
    return this.localStream;
  }

  /** Snapshot of current client state + WebRTC stats. For the debug panel. */
  async getDiagnostics(): Promise<VideoDiagnostics> {
    const pc = this.pc;
    const senders: TrackInfo[] = pc
      ? pc.getSenders().map((s) => ({
          kind: s.track?.kind,
          enabled: s.track?.enabled,
          muted: s.track?.muted,
          readyState: s.track?.readyState,
          trackId: s.track?.id,
        }))
      : [];
    const receivers: TrackInfo[] = pc
      ? pc.getReceivers().map((r) => ({
          kind: r.track?.kind,
          enabled: r.track?.enabled,
          muted: r.track?.muted,
          readyState: r.track?.readyState,
          trackId: r.track?.id,
        }))
      : [];

    let stats: Array<Record<string, unknown>> = [];
    if (pc) {
      try {
        const raw = await pc.getStats();
        // Keep only entries that are useful for live-call debugging.
        const KEEP = new Set([
          "inbound-rtp",
          "outbound-rtp",
          "remote-inbound-rtp",
          "remote-outbound-rtp",
          "candidate-pair",
          "local-candidate",
          "remote-candidate",
          "transport",
        ]);
        raw.forEach((report) => {
          if (KEEP.has(report.type)) {
            // Flatten RTCStats to plain object for JSON serialization.
            stats.push({ ...report });
          }
        });
      } catch (e) {
        stats = [{ error: String(e) }];
      }
    }

    return {
      userId: this.opts.userId,
      sessionId: this.opts.sessionId,
      participantId: this.participantId,
      joined: this.joined,
      micEnabled: this.micEnabled,
      camEnabled: this.camEnabled,
      pcConnectionState: pc?.connectionState ?? "none",
      iceConnectionState: pc?.iceConnectionState ?? "none",
      iceGatheringState: pc?.iceGatheringState ?? "none",
      signalingState: pc?.signalingState ?? "none",
      localDescriptionType: pc?.localDescription?.type,
      remoteDescriptionType: pc?.remoteDescription?.type,
      senders,
      receivers,
      // this.participants is keyed by both participantId and per-stream aliases
      // (from offer.tracks mapping), so values() yields the same Participant
      // multiple times. Dedupe for the dump so the diagnostic list matches
      // what the UI actually renders.
      participants: Array.from(
        new Map(
          Array.from(this.participants.values()).map((p) => [p.participantId, p]),
        ).values(),
      ).map((p) => ({
        participantId: p.participantId,
        userId: p.userId,
        hasAudio: !!p.audioTrack,
        hasVideo: !!p.videoTrack,
        isSpeaking: p.isSpeaking,
        isMicMuted: p.isMicMuted,
      })),
      stats,
    };
  }

  // ── Cleanup ────────────────────────────────────

  /** Disconnect and clean up everything */
  disconnect() {
    if (this.speakingInterval) {
      clearInterval(this.speakingInterval);
      this.speakingInterval = null;
    }
    this.audioContext?.close();
    this.audioContext = null;
    this.analyserNodes.clear();
    this.vadSilenceTicks.clear();

    if (this.ws?.readyState === WebSocket.OPEN) {
      this.send({ type: "leave" });
      this.ws.close();
    }
    this.ws = null;

    this.localStream?.getTracks().forEach((t) => t.stop());
    this.localStream = null;

    this.pc?.close();
    this.pc = null;
    this.videoSender = null;
    this.audioSender = null;
    this.currentMicDeviceId = null;
    this.currentCameraDeviceId = null;
    this.screenVideoTrack?.stop();
    this.screenAudioTrack?.stop();
    this.screenVideoTrack = null;
    this.screenAudioTrack = null;
    this.screenVideoSender = null;
    this.screenAudioSender = null;
    this.streamMeta.clear();
    this.msgQueue = Promise.resolve();

    this.participants.clear();
  }
}
