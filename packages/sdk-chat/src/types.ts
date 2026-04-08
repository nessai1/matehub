export interface ChatClientOptions {
  /** WebSocket URL for chat (e.g. wss://chat.matehub.io/ws) */
  wsUrl: string;
  /** JWT token */
  token: string;
}

export interface ChannelInfo {
  channelId: string;
  name: string;
  type: "text" | "voice" | "stage";
}

export interface Message {
  messageId: string;
  channelId: string;
  userId: string;
  content: string;
  timestamp: number;
  type: "user" | "system";
}
