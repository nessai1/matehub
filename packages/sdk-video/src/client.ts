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
  private pendingCandidates: Array<{ candidate: string; sdpMid: string | null; sdpMLineIndex: number | null }> = [];
  private joined = false;

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

    // Send ICE candidates to server (buffer until joined)
    pc.onicecandidate = (e) => {
      if (e.candidate) {
        this.log("ICE candidate (local):", e.candidate.candidate);
        const candidate = {
          candidate: e.candidate.candidate,
          sdpMid: e.candidate.sdpMid,
          sdpMLineIndex: e.candidate.sdpMLineIndex,
        };
        if (this.joined) {
          this.send({
            type: "ice_candidate",
            candidate: candidate.candidate,
            sdp_mid: candidate.sdpMid,
            sdp_mline_index: candidate.sdpMLineIndex,
          });
        } else {
          this.log("buffering ICE candidate (not yet joined)");
          this.pendingCandidates.push(candidate);
        }
      } else {
        this.log("ICE gathering complete");
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

        // Listen for mute/unmute on remote video track.
        // Browser fires these when media data stops/resumes arriving.
        e.track.onmute = () => {
          this.log("remote video track muted", { participantId: participant!.participantId });
          this.emit({
            type: "track_muted",
            participantId: participant!.participantId,
            trackKind: "video",
            muted: true,
          });
        };
        e.track.onunmute = () => {
          this.log("remote video track unmuted", { participantId: participant!.participantId });
          this.emit({
            type: "track_muted",
            participantId: participant!.participantId,
            trackKind: "video",
            muted: false,
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
        pc.addTrack(track, stream);
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

    // Wait for ICE gathering to complete so we can send all candidates with the offer.
    // This avoids the race where str0m starts ICE checking before candidates arrive.
    if (pc.iceGatheringState !== "complete") {
      this.log("waiting for ICE gathering to complete...");
      await new Promise<void>((resolve) => {
        const check = () => {
          if (pc.iceGatheringState === "complete") {
            resolve();
          }
        };
        pc.onicegatheringstatechange = () => {
          this.log("ICE gathering state:", pc.iceGatheringState);
          check();
        };
        check(); // in case it's already complete
        // Safety timeout -- don't wait forever
        setTimeout(resolve, 5000);
      });
    }

    // localDescription now contains all ICE candidates inline
    const sdpWithCandidates = pc.localDescription?.sdp ?? offer.sdp;
    this.log("sending join with complete SDP (candidates inline)", {
      candidateCount: (sdpWithCandidates?.match(/a=candidate:/g) || []).length,
    });

    this.send({
      type: "join",
      sdp_offer: sdpWithCandidates,
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

        // Flush buffered ICE candidates
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
