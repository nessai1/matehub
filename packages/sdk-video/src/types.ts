export interface VideoClientOptions {
  /** WebSocket URL for signaling (e.g. wss://video.matehub.io/ws) */
  wsUrl: string;
  /** JWT participant token from POST /v1/token */
  token: string;
  /** ICE servers (STUN/TURN) */
  iceServers?: RTCIceServer[];
}

export interface SessionInfo {
  sessionId: string;
  channelId: string;
  participants: ParticipantInfo[];
}

export interface ParticipantInfo {
  userId: string;
  displayName: string;
  tracks: TrackInfo[];
  isSpeaking: boolean;
}

export interface TrackInfo {
  trackId: string;
  kind: "audio" | "video" | "screen";
  muted: boolean;
}
