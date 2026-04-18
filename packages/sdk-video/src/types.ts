export interface VideoClientOptions {
  /** Video service base URL (e.g. http://localhost:4000) */
  serverUrl: string;
  /** Session ID to connect to */
  sessionId: string;
  /** User identifier */
  userId: string;
  /** Auth token (dev mode: "dev-alice-token") */
  token: string;
  /** ICE servers (STUN/TURN). Empty array for local dev. */
  iceServers?: RTCIceServer[];
}

export interface Participant {
  participantId: string;
  userId: string;
  audioTrack: MediaStreamTrack | null;
  videoTrack: MediaStreamTrack | null;
  isSpeaking: boolean;
  isMicMuted: boolean;
  stream: MediaStream;
}

export type VideoClientEvent =
  | { type: "connected"; participantId: string }
  | { type: "participant_joined"; participant: Participant }
  | { type: "participant_left"; participantId: string; userId: string }
  | { type: "track_added"; participantId: string; track: MediaStreamTrack; stream: MediaStream }
  | { type: "track_muted"; participantId: string; trackKind: string; muted: boolean }
  | { type: "speaking_changed"; participantId: string; speaking: boolean }
  | { type: "disconnected"; reason: string }
  | { type: "error"; message: string }
  | { type: "debug"; level: "info" | "warn" | "error"; msg: string; data?: unknown };

export interface TrackInfo {
  kind: string | undefined;
  enabled: boolean | undefined;
  muted: boolean | undefined;
  readyState: string | undefined;
  trackId: string | undefined;
}

export interface VideoDiagnostics {
  /** Local user identifier (so the dump says who this client is). */
  userId: string;
  /** Session this client is connected to. */
  sessionId: string;
  participantId: string | null;
  joined: boolean;
  micEnabled: boolean;
  camEnabled: boolean;
  pcConnectionState: RTCPeerConnectionState | "none";
  iceConnectionState: RTCIceConnectionState | "none";
  iceGatheringState: RTCIceGathererState | "none";
  signalingState: RTCSignalingState | "none";
  localDescriptionType: string | undefined;
  remoteDescriptionType: string | undefined;
  senders: TrackInfo[];
  receivers: TrackInfo[];
  participants: Array<{
    participantId: string;
    userId: string;
    hasAudio: boolean;
    hasVideo: boolean;
    isSpeaking: boolean;
    isMicMuted: boolean;
  }>;
  stats: Array<Record<string, unknown>>;
}
