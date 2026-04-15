import type { VideoClientOptions, VideoClientEvent, Participant } from "./types";

type EventHandler = (event: VideoClientEvent) => void;

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
  /** Sequential processing queue -- prevents concurrent setRemoteDescription calls */
  private msgQueue: Promise<void> = Promise.resolve();

  // Audio level detection
  private audioContext: AudioContext | null = null;
  private analyserNodes = new Map<string, AnalyserNode>();
  private speakingInterval: ReturnType<typeof setInterval> | null = null;

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
  }

  /** Connect to the session: open WebSocket, create PeerConnection */
  async connect() {
    // Prevent double connect (React Strict Mode, HMR)
    if (this.ws || this.pc) {
      this.log("connect called but already connected, ignoring");
      return;
    }

    const wsProto = this.opts.serverUrl.startsWith("https") ? "wss" : "ws";
    const host = this.opts.serverUrl.replace(/^https?:\/\//, "");
    const wsUrl = `${wsProto}://${host}/ws/${this.opts.sessionId}?user_id=${this.opts.userId}&token=${this.opts.token}`;

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

    this.ws.onerror = (e) => {
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
      this.log("ICE connection state:", pc.iceConnectionState);
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
      let participant = this.participants.get(streamId);

      if (!participant) {
        // Fallback: create placeholder (shouldn't happen if offer has tracks mapping)
        this.log("creating placeholder participant for stream (no mapping)", { streamId });
        participant = {
          participantId: streamId,
          userId: "remote",
          audioTrack: null,
          videoTrack: null,
          isSpeaking: false,
          isMicMuted: true,
          stream,
        };
        this.participants.set(streamId, participant);
        this.emit({ type: "participant_joined", participant });
      }

      if (e.track.kind === "audio") {
        participant.audioTrack = e.track;
        this.setupAudioLevelDetection(participant.participantId, e.track);
      } else if (e.track.kind === "video") {
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
      }

      this.emit({
        type: "track_added",
        participantId: participant.participantId,
        track: e.track,
        stream,
      });
    };

    pc.onconnectionstatechange = () => {
      if (pc.connectionState === "failed" || pc.connectionState === "disconnected") {
        this.emit({ type: "disconnected", reason: `peer connection ${pc.connectionState}` });
      }
    };
  }

  private async createAndSendOffer() {
    const pc = this.pc!;

    // Get user media BEFORE creating offer so tracks are in the SDP.
    // This ensures str0m sees actual sending tracks, not empty sendrecv transceivers.
    // Tracks are added but MUTED by default -- user enables them explicitly.
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: true,
        video: { width: 640, height: 480 },
      });
      this.localStream = stream;

      for (const track of stream.getTracks()) {
        track.enabled = false; // muted by default
        const sender = pc.addTrack(track, stream);
        if (track.kind === "video") {
          this.videoSender = sender;
          // track.enabled=false still sends black-frame RTP, which triggers
          // onunmute on the receiver -> grey tile instead of avatar.
          // replaceTrack(null) stops RTP entirely -> receiver track stays muted.
          sender.replaceTrack(null);
        }
        this.log("added local track to PC (muted)", { kind: track.kind, id: track.id });
      }

      this.micEnabled = false;
      this.camEnabled = false;
    } catch (e) {
      this.log("getUserMedia failed, falling back to recvonly", e);
      // Fallback: receive-only if no camera/mic available
      pc.addTransceiver("audio", { direction: "recvonly" });
      pc.addTransceiver("video", { direction: "recvonly" });
    }

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);

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
        // Register stream_id -> participant mapping from SFU
        const tracks = msg.tracks as Array<{
          stream_id: string;
          participant_id: string;
          user_id: string;
        }> | undefined;

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
          }
        }

        // SFU renegotiation (new tracks available)
        await this.pc!.setRemoteDescription({
          type: "offer",
          sdp: msg.sdp_offer as string,
        });
        const answer = await this.pc!.createAnswer();
        await this.pc!.setLocalDescription(answer);
        this.send({
          type: "answer",
          sdp_answer: answer.sdp,
        });
        break;
      }

      case "ice_candidate": {
        await this.pc!.addIceCandidate({
          candidate: msg.candidate as string,
          sdpMid: msg.sdp_mid as string | null,
          sdpMLineIndex: msg.sdp_mline_index as number | null,
        });
        break;
      }

      case "participant_joined": {
        const p: Participant = {
          participantId: msg.participant_id as string,
          userId: msg.user_id as string,
          audioTrack: null,
          videoTrack: null,
          isSpeaking: false,
          isMicMuted: true,
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
        this.emit({
          type: "track_muted",
          participantId: pid,
          trackKind: kind,
          muted,
        });
        break;
      }

      case "error": {
        this.emit({ type: "error", message: msg.message as string });
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
        this.pc.addTrack(track, this.localStream);
        this.log("enableMic added track to PC, senders:", this.pc.getSenders().length);
      }
    }
    this.micEnabled = true;
    this.send({ type: "mute_changed", kind: "audio", muted: false });
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

    if (existingTrack && existingTrack.readyState === "live") {
      existingTrack.enabled = true;
      await this.videoSender?.replaceTrack(existingTrack);
      this.log("enableCamera re-enabled existing track via replaceTrack");
    } else {
      const stream = await navigator.mediaDevices.getUserMedia({
        video: { width: 640, height: 480 },
      });
      const track = stream.getVideoTracks()[0];
      this.log("enableCamera acquired new track", { trackId: track?.id });
      if (track && this.pc) {
        if (!this.localStream) this.localStream = new MediaStream();
        this.localStream.addTrack(track);
        if (this.videoSender) {
          await this.videoSender.replaceTrack(track);
        } else {
          this.videoSender = this.pc.addTrack(track, this.localStream);
        }
        this.log("enableCamera track on PC, senders:", this.pc.getSenders().length);
      }
    }
    this.camEnabled = true;
    this.send({ type: "mute_changed", kind: "video", muted: false });
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

  get isMicEnabled() {
    return this.micEnabled;
  }

  get isCamEnabled() {
    return this.camEnabled;
  }

  /** Start screen sharing */
  async startScreenShare() {
    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: true,
      audio: false,
    });
    const track = stream.getVideoTracks()[0];
    if (track && this.pc) {
      this.pc.addTrack(track, stream);
      await this.renegotiate();

      // Auto-stop when user clicks "Stop sharing" in browser UI
      track.onended = () => {
        this.stopScreenShare();
      };
    }
    return stream;
  }

  /** Stop screen sharing */
  stopScreenShare() {
    // Remove screen share tracks
    // This would need renegotiation in a full implementation
  }

  private async acquireLocalStream(constraints: MediaStreamConstraints): Promise<MediaStream> {
    if (!this.localStream) {
      this.localStream = await navigator.mediaDevices.getUserMedia(constraints);
    } else {
      // Add new tracks to existing stream
      const newStream = await navigator.mediaDevices.getUserMedia(constraints);
      for (const track of newStream.getTracks()) {
        this.localStream.addTrack(track);
      }
    }
    return this.localStream;
  }

  private async renegotiate() {
    if (!this.pc || !this.ws) return;
    // SFU-initiated renegotiation handles this via offer from server
    // For client-initiated changes, we'd need to send a new offer
    // For now, the SFU will detect new tracks and renegotiate
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
    const THRESHOLD = 15; // lowered: catches quiet speech
    const data = new Uint8Array(256); // fftSize/2

    for (const [pid, analyser] of this.analyserNodes) {
      analyser.getByteFrequencyData(data);

      // Average only the voice-frequency bins, not the whole spectrum
      let sum = 0;
      for (let i = VOICE_BIN_START; i < VOICE_BIN_END; i++) {
        sum += data[i];
      }
      const avg = sum / (VOICE_BIN_END - VOICE_BIN_START);
      const speaking = avg > THRESHOLD;

      const participant = this.participants.get(pid);
      if (participant && participant.isSpeaking !== speaking) {
        participant.isSpeaking = speaking;
        this.emit({ type: "speaking_changed", participantId: pid, speaking });
      }
    }
  }

  private findParticipantByStreamId(streamId: string): Participant | undefined {
    // SFU sets stream ID to origin participant ID
    return this.participants.get(streamId);
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
    this.msgQueue = Promise.resolve();

    this.participants.clear();
  }
}
