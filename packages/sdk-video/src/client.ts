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
        this.handleServerMessage(msg);
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

    // Send ICE candidates to server
    pc.onicecandidate = (e) => {
      if (e.candidate) {
        this.send({
          type: "ice_candidate",
          candidate: e.candidate.candidate,
          sdp_mid: e.candidate.sdpMid,
          sdp_mline_index: e.candidate.sdpMLineIndex,
        });
      }
    };

    // Handle remote tracks (from other participants via SFU)
    pc.ontrack = (e) => {
      const stream = e.streams[0] ?? new MediaStream([e.track]);
      const streamId = stream.id;

      // streamId maps to the origin participant (SFU sets this)
      let participant = this.findParticipantByStreamId(streamId);
      if (!participant) {
        // Create placeholder -- will be matched when we get participant info
        participant = {
          participantId: streamId,
          userId: streamId,
          audioTrack: null,
          videoTrack: null,
          isSpeaking: false,
          stream,
        };
        this.participants.set(streamId, participant);
      }

      if (e.track.kind === "audio") {
        participant.audioTrack = e.track;
        this.setupAudioLevelDetection(participant.participantId, e.track);
      } else if (e.track.kind === "video") {
        participant.videoTrack = e.track;
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

    // Add transceivers for receiving audio/video even if we're not sending yet
    pc.addTransceiver("audio", { direction: "recvonly" });
    pc.addTransceiver("video", { direction: "recvonly" });

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);

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
        this.emit({ type: "connected", participantId: this.participantId });
        break;
      }

      case "offer": {
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
  }

  /** Disable microphone (mutes track, keeps it in PeerConnection) */
  disableMic() {
    const track = this.localStream?.getAudioTracks()[0];
    this.log("disableMic", { trackExists: !!track, enabled: track?.enabled, readyState: track?.readyState });
    if (track) track.enabled = false;
    this.micEnabled = false;
  }

  /** Enable camera: first call acquires device, subsequent calls re-enable track */
  async enableCamera() {
    this.log("enableCamera", { camEnabled: this.camEnabled, hasLocalStream: !!this.localStream });
    if (this.camEnabled) return;

    const existingTrack = this.localStream?.getVideoTracks()[0];
    this.log("enableCamera existingTrack", { exists: !!existingTrack, enabled: existingTrack?.enabled, readyState: existingTrack?.readyState });

    if (existingTrack && existingTrack.readyState === "live") {
      existingTrack.enabled = true;
      this.log("enableCamera re-enabled existing track");
    } else {
      const stream = await navigator.mediaDevices.getUserMedia({
        video: { width: 640, height: 480 },
      });
      const track = stream.getVideoTracks()[0];
      this.log("enableCamera acquired new track", { trackId: track?.id });
      if (track && this.pc) {
        if (!this.localStream) this.localStream = new MediaStream();
        this.localStream.addTrack(track);
        this.pc.addTrack(track, this.localStream);
        this.log("enableCamera added track to PC, senders:", this.pc.getSenders().length);
      }
    }
    this.camEnabled = true;
  }

  /** Disable camera (mutes track, keeps it in PeerConnection) */
  disableCamera() {
    const track = this.localStream?.getVideoTracks()[0];
    this.log("disableCamera", { trackExists: !!track, enabled: track?.enabled, readyState: track?.readyState });
    if (track) track.enabled = false;
    this.camEnabled = false;
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
    analyser.fftSize = 256;
    source.connect(analyser);
    this.analyserNodes.set(participantId, analyser);

    if (!this.speakingInterval) {
      this.speakingInterval = setInterval(() => this.checkSpeakingLevels(), 200);
    }
  }

  private checkSpeakingLevels() {
    const threshold = 30; // audio level threshold for "speaking"
    const data = new Uint8Array(128);

    for (const [pid, analyser] of this.analyserNodes) {
      analyser.getByteFrequencyData(data);
      const avg = data.reduce((sum, val) => sum + val, 0) / data.length;
      const speaking = avg > threshold;

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

    this.participants.clear();
  }
}
