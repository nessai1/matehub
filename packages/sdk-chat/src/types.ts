// ── Client options ───────────────────────────────

export interface ChatClientOptions {
  /** Chat service base URL (e.g. http://localhost:3003) */
  baseUrl: string;
  /** JWT access token */
  token: string;
  /**
   * Hub ID this client is scoped to. Snowflakes exceed JS safe-integer
   * range, so every entity id on the wire is a decimal string.
   */
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

// ── Attachments ──────────────────────────────────

export type AttachmentStatus = "ready" | "transcoding" | "failed";

export interface Attachment {
  id: string;
  url: string;
  content_type: string;
  name: string;
  size: number;
  width?: number;
  height?: number;
  duration?: number;
  thumb_url?: string;
  status?: AttachmentStatus;
}

export type AttachmentKind = "image" | "video" | "audio" | "document";

export function attachmentKind(a: Attachment): AttachmentKind {
  if (a.content_type.startsWith("image/")) return "image";
  if (a.content_type.startsWith("video/")) return "video";
  if (a.content_type.startsWith("audio/")) return "audio";
  return "document";
}

// ── Domain models ────────────────────────────────

/**
 * Snowflake ID fields are **strings** — the numbers exceed
 * `Number.MAX_SAFE_INTEGER` and `JSON.parse` would silently round them.
 * Comparisons with `===`, interpolation into URLs, and Map keys all work
 * naturally on strings.
 */
export interface Message {
  hub_id: string;
  channel_id: string;
  message_id: string;
  author_id: string;
  author_type: string;
  content: string;
  thread_root_id: string | null;
  mentions: string[];
  mention_groups: string[];
  mention_everyone: boolean;
  attachments: Attachment[];
  edited_at: string | null;
  deleted_at: string | null;
  client_id: string | null;
  bucket: number;
}

export interface TypingEvent {
  user_id: string;
  channel_id: string;
}

// ── Events (discriminated union, same pattern as VideoClient) ──

export type ChatClientEvent =
  | { type: "connection.state"; state: ConnectionState }
  | { type: "ready"; sessionId: string; userId: string; username: string }
  | { type: "resumed"; replayedCount: number }
  | { type: "message.new"; message: Message }
  | { type: "message.updated"; data: MessageUpdateData }
  | { type: "message.deleted"; data: MessageDeleteData }
  | { type: "attachment.updated"; data: AttachmentUpdatedData }
  | { type: "typing.start"; data: TypingEvent }
  | { type: "error"; message: string; code?: number };

export interface AttachmentUpdatedData {
  message_id: string;
  channel_id: string;
  attachment_id: string;
  attachments: Attachment[];
}

export interface MessageUpdateData {
  message_id: string;
  channel_id: string;
  content: string;
  edited: boolean;
}

export interface MessageDeleteData {
  message_id: string;
  channel_id: string;
}

// ── REST request/response types ──────────────────

export interface SendMessageOptions {
  content: string;
  clientId?: string;
  threadRootId?: string;
  attachments?: Attachment[];
}

export interface HistoryOptions {
  limit?: number;
  before?: string;
}

export interface SyncChannelRequest {
  channel_id: string;
  after: string;
}

export interface SyncChannelResponse {
  channel_id: string;
  messages: Message[];
  limited: boolean;
}

export interface SyncResponse {
  channels: SyncChannelResponse[];
}

// ── Read State ───────────────────────────────────

export interface ChannelReadState {
  channel_id: string;
  last_read_message_id: string;
  mention_count: number;
}
