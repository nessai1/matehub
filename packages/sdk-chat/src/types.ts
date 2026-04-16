// ── Client options ───────────────────────────────

export interface ChatClientOptions {
  /** Chat service base URL (e.g. http://localhost:3003) */
  baseUrl: string;
  /** JWT access token */
  token: string;
  /** Hub ID this client is scoped to */
  hubId: string;
}

// ── Connection state ─────────────────────────────

export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "identifying"
  | "connected"
  | "resuming"
  | "reconnecting";

// ── Domain models ────────────────────────────────

export interface Message {
  hub_id: number;
  channel_id: number;
  message_id: number;
  author_id: string;
  author_type: string;
  content: string;
  thread_root_id: number | null;
  mentions: string[];
  mention_groups: string[];
  mention_everyone: boolean;
  attachments: string[];
  edited_at: string | null;
  deleted_at: string | null;
  client_id: string | null;
  bucket: number;
}

export interface TypingEvent {
  user_id: string;
  channel_id: number;
}

// ── Events (discriminated union, same pattern as VideoClient) ──

export type ChatClientEvent =
  | { type: "connection.state"; state: ConnectionState }
  | { type: "ready"; sessionId: string; userId: string; username: string }
  | { type: "resumed"; replayedCount: number }
  | { type: "message.new"; message: Message }
  | { type: "message.updated"; data: MessageUpdateData }
  | { type: "message.deleted"; data: MessageDeleteData }
  | { type: "typing.start"; data: TypingEvent }
  | { type: "error"; message: string; code?: number };

export interface MessageUpdateData {
  message_id: number;
  channel_id: number;
  content: string;
  edited: boolean;
}

export interface MessageDeleteData {
  message_id: number;
  channel_id: number;
}

// ── REST request/response types ──────────────────

export interface SendMessageOptions {
  content: string;
  clientId?: string;
  threadRootId?: number;
  attachments?: string[];
}

export interface HistoryOptions {
  limit?: number;
  before?: number;
}

export interface SyncChannelRequest {
  channel_id: number;
  after: number;
}

export interface SyncChannelResponse {
  channel_id: number;
  messages: Message[];
  limited: boolean;
}

export interface SyncResponse {
  channels: SyncChannelResponse[];
}
