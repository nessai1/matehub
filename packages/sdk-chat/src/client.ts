import type {
  ChatClientOptions,
  ChatClientEvent,
  ConnectionState,
  Message,
  SendMessageOptions,
  HistoryOptions,
  SyncChannelRequest,
  SyncResponse,
  Attachment,
  ChannelReadState,
} from "./types";

type EventHandler = (event: ChatClientEvent) => void;

/**
 * Gateway opcodes (must match Rust Opcode enum).
 */
const Op = {
  Dispatch: 0,
  Heartbeat: 1,
  Identify: 2,
  Resume: 6,
  Reconnect: 7,
  InvalidSession: 9,
  Hello: 10,
  HeartbeatAck: 11,
} as const;

/**
 * Decorrelated jitter backoff (AWS Architecture Blog).
 * Avoids thundering herd on mass reconnect -- each client picks a
 * genuinely random delay in [base, prev*3], not 2^n which clusters.
 */
function nextDelay(prev: number, base = 1000, cap = 30_000): number {
  return Math.min(cap, Math.floor(base + Math.random() * (prev * 3 - base)));
}

/**
 * MateHub Chat SDK client.
 *
 * Manages WebSocket connection to the chat gateway (fastwebsockets),
 * auto-heartbeat, RESUME on reconnect, and REST API calls.
 *
 * Usage:
 *   const chat = new ChatClient({ baseUrl, token, hubId });
 *   chat.on(event => { ... });
 *   chat.connect();
 *   await chat.sendMessage(channelId, { content: "hello" });
 */
export class ChatClient {
  private opts: ChatClientOptions;
  private handlers: EventHandler[] = [];
  private ws: WebSocket | null = null;

  // ── Connection state ───────────────────────────
  private _state: ConnectionState = "disconnected";
  private sessionId: string | null = null;
  private lastSeq = 0;
  private heartbeatInterval: ReturnType<typeof setInterval> | null = null;
  private heartbeatAckPending = false;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectDelay = 1000;
  private reconnectAttempts = 0;
  private intentionalClose = false;

  constructor(opts: ChatClientOptions) {
    this.opts = opts;
  }

  // ── Event system (same pattern as VideoClient) ──

  /** Subscribe to events. Returns unsubscribe function. */
  on(handler: EventHandler): () => void {
    this.handlers.push(handler);
    return () => {
      this.handlers = this.handlers.filter((h) => h !== handler);
    };
  }

  private emit(event: ChatClientEvent) {
    for (const handler of this.handlers) {
      try {
        handler(event);
      } catch (e) {
        console.error("[ChatClient] event handler error:", e);
      }
    }
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private log(msg: string, ...args: any[]) {
    console.log(`[ChatClient] ${msg}`, ...args);
  }

  private setState(state: ConnectionState) {
    if (this._state === state) return;
    this._state = state;
    this.emit({ type: "connection.state", state });
  }

  get state(): ConnectionState {
    return this._state;
  }

  // ── WebSocket lifecycle ────────────────────────

  /** Open WebSocket connection to the gateway. */
  connect() {
    if (this.ws) {
      this.log("connect() called but already connected, ignoring");
      return;
    }
    this.intentionalClose = false;
    this.openWebSocket();
  }

  /** Graceful disconnect. Clears session -- no RESUME on next connect(). */
  disconnect() {
    this.intentionalClose = true;
    this.clearTimers();
    this.sessionId = null;
    this.lastSeq = 0;

    if (this.ws) {
      // Null handlers BEFORE close to prevent processing queued frames
      // (React StrictMode: cleanup runs while HELLO is still in event queue)
      this.ws.onmessage = null;
      this.ws.onclose = null;
      this.ws.onerror = null;
      if (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING) {
        this.ws.close(1000, "client disconnect");
      }
      this.ws = null;
    }
    this.setState("disconnected");
  }

  /** Update token (e.g. after JWT refresh). Takes effect on next reconnect. */
  updateToken(token: string) {
    this.opts = { ...this.opts, token };
  }

  private openWebSocket() {
    // baseUrl can be absolute ("https://chat.example.com") or relative
    // ("/api/chat" for same-origin). The URL constructor handles both: a
    // relative second arg gets resolved against the first; an absolute one
    // overrides it. Then we just swap http(s) → ws(s).
    const base =
      typeof window !== "undefined"
        ? window.location.href
        : "http://localhost";
    const url = new URL(`${this.opts.baseUrl}/gateway`, base);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    const wsUrl = url.toString();

    this.log("opening WS", wsUrl);
    this.setState(this.sessionId ? "resuming" : "connecting");

    const ws = new WebSocket(wsUrl);
    this.ws = ws;

    ws.onopen = () => {
      this.log("WS opened, waiting for HELLO");
    };

    ws.onmessage = (e) => {
      try {
        const frame = JSON.parse(e.data as string);
        this.handleFrame(frame);
      } catch (err) {
        console.error("[ChatClient] failed to parse frame:", err);
      }
    };

    ws.onclose = (e) => {
      this.log("WS closed", { code: e.code, reason: e.reason });
      this.ws = null;
      this.clearHeartbeat();

      if (!this.intentionalClose) {
        this.scheduleReconnect();
      } else {
        this.setState("disconnected");
      }
    };

    ws.onerror = () => {
      this.emit({ type: "error", message: "WebSocket error" });
    };
  }

  // ── Frame handling ─────────────────────────────

  private handleFrame(frame: { op: number; s?: number; t?: string; d: unknown }) {
    if (this.intentionalClose) return;
    switch (frame.op) {
      case Op.Hello:
        this.handleHello(frame.d as { heartbeat_interval: number });
        break;

      case Op.Dispatch:
        if (frame.s != null) this.lastSeq = frame.s;
        this.handleDispatch(frame.t!, frame.d);
        break;

      case Op.HeartbeatAck:
        this.heartbeatAckPending = false;
        break;

      case Op.InvalidSession: {
        const resumable = frame.d as boolean;
        this.log("INVALID_SESSION", { resumable });
        if (!resumable) {
          // Session gone -- fresh IDENTIFY on reconnect
          this.sessionId = null;
          this.lastSeq = 0;
        }
        // Server will close the connection; onclose triggers reconnect
        break;
      }

      case Op.Reconnect:
        this.log("server requested RECONNECT");
        this.ws?.close(4000, "server reconnect");
        break;

      default:
        this.log("unknown op", frame.op);
    }
  }

  private handleHello(data: { heartbeat_interval: number }) {
    this.log("HELLO", { interval: data.heartbeat_interval });
    this.startHeartbeat(data.heartbeat_interval);

    if (this.sessionId) {
      // RESUME
      this.setState("resuming");
      this.sendFrame(Op.Resume, {
        session_id: this.sessionId,
        seq: this.lastSeq,
      });
    } else {
      // Fresh IDENTIFY
      this.setState("identifying");
      this.sendFrame(Op.Identify, {
        token: this.opts.token,
      });
    }
  }

  private handleDispatch(eventType: string, data: unknown) {
    switch (eventType) {
      case "READY": {
        const d = data as { session_id: string; user: { id: string; username: string } };
        this.sessionId = d.session_id;
        this.reconnectAttempts = 0;
        this.reconnectDelay = 1000;
        this.setState("connected");
        this.emit({
          type: "ready",
          sessionId: d.session_id,
          userId: d.user.id,
          username: d.user.username,
        });
        break;
      }

      case "RESUMED": {
        this.reconnectAttempts = 0;
        this.reconnectDelay = 1000;
        this.setState("connected");
        this.emit({ type: "resumed", replayedCount: 0 });
        break;
      }

      case "MESSAGE_CREATE":
        this.emit({ type: "message.new", message: data as Message });
        break;

      case "MESSAGE_UPDATE":
        this.emit({
          type: "message.updated",
          data: data as { message_id: string; channel_id: string; content: string; edited: boolean },
        });
        break;

      case "MESSAGE_DELETE":
        this.emit({
          type: "message.deleted",
          data: data as { message_id: string; channel_id: string },
        });
        break;

      case "ATTACHMENT_UPDATED":
        this.emit({
          type: "attachment.updated",
          data: data as { message_id: string; channel_id: string; attachment_id: string; attachments: Attachment[] },
        });
        break;

      case "TYPING_START":
        this.emit({
          type: "typing.start",
          data: data as { user_id: string; channel_id: string },
        });
        break;

      case "DM_CALL_INVITE":
        this.emit({
          type: "dm.call.invite",
          data: data as { from_user_id: string; channel_id: string },
        });
        break;

      case "DM_CALL_DECLINE":
        this.emit({
          type: "dm.call.decline",
          data: data as { from_user_id: string; channel_id: string },
        });
        break;

      case "DM_CALL_CANCEL":
        this.emit({
          type: "dm.call.cancel",
          data: data as { from_user_id: string; channel_id: string },
        });
        break;

      case "DM_CALL_ENDED":
        this.emit({
          type: "dm.call.ended",
          data: data as {
            from_user_id: string;
            channel_id: string;
            duration_secs: number;
          },
        });
        break;

      default:
        this.log("unhandled dispatch", eventType);
    }
  }

  // ── Heartbeat ──────────────────────────────────

  private startHeartbeat(intervalMs: number) {
    this.clearHeartbeat();
    this.heartbeatAckPending = false;

    // Jittered first heartbeat (anti-thundering-herd, Discord pattern)
    const firstDelay = Math.floor(intervalMs * Math.random());

    setTimeout(() => {
      this.sendHeartbeat();
      this.heartbeatInterval = setInterval(() => {
        if (this.heartbeatAckPending) {
          // Missed ACK -> zombie connection
          this.log("heartbeat ACK missed, reconnecting");
          this.ws?.close(4000, "heartbeat timeout");
          return;
        }
        this.sendHeartbeat();
      }, intervalMs);
    }, firstDelay);
  }

  private sendHeartbeat() {
    this.heartbeatAckPending = true;
    this.sendFrame(Op.Heartbeat, this.lastSeq);
  }

  private clearHeartbeat() {
    if (this.heartbeatInterval) {
      clearInterval(this.heartbeatInterval);
      this.heartbeatInterval = null;
    }
  }

  // ── Reconnect with decorrelated jitter ─────────

  private scheduleReconnect() {
    this.setState("reconnecting");
    this.reconnectAttempts++;
    this.reconnectDelay = nextDelay(this.reconnectDelay);

    this.log("reconnecting", {
      attempt: this.reconnectAttempts,
      delay: this.reconnectDelay,
      hasSession: !!this.sessionId,
    });

    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.openWebSocket();
    }, this.reconnectDelay);
  }

  private clearTimers() {
    this.clearHeartbeat();
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  // ── Low-level WS send ──────────────────────────

  private sendFrame(op: number, data: unknown) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify({ op, d: data }));
    }
  }

  // ── REST API methods ───────────────────────────

  private get apiBase(): string {
    return this.opts.baseUrl;
  }

  private get headers(): Record<string, string> {
    return {
      "Content-Type": "application/json",
      Authorization: `Bearer ${this.opts.token}`,
    };
  }

  /** Send a message to a channel. */
  async sendMessage(channelId: string, opts: SendMessageOptions): Promise<Message> {
    const res = await fetch(`${this.apiBase}/v1/channels/${channelId}/messages`, {
      method: "POST",
      headers: this.headers,
      body: JSON.stringify({
        content: opts.content,
        client_id: opts.clientId,
        thread_root_id: opts.threadRootId,
        attachments: opts.attachments,
      }),
    });
    if (!res.ok) throw await chatError(res);
    return res.json();
  }

  /** Edit a message. */
  async editMessage(channelId: string, messageId: string, content: string): Promise<void> {
    const res = await fetch(
      `${this.apiBase}/v1/channels/${channelId}/messages/${messageId}`,
      {
        method: "PATCH",
        headers: this.headers,
        body: JSON.stringify({ content }),
      },
    );
    if (!res.ok) throw await chatError(res);
  }

  /** Delete a message. */
  async deleteMessage(channelId: string, messageId: string): Promise<void> {
    const res = await fetch(
      `${this.apiBase}/v1/channels/${channelId}/messages/${messageId}`,
      {
        method: "DELETE",
        headers: this.headers,
      },
    );
    if (!res.ok) throw await chatError(res);
  }

  /** Fetch message history (newest first). */
  async getHistory(channelId: string, opts?: HistoryOptions): Promise<Message[]> {
    const params = new URLSearchParams();
    if (opts?.limit) params.set("limit", String(opts.limit));
    if (opts?.before) params.set("before", String(opts.before));
    const qs = params.toString();

    const res = await fetch(
      `${this.apiBase}/v1/channels/${channelId}/messages${qs ? `?${qs}` : ""}`,
      { headers: this.headers },
    );
    if (!res.ok) throw await chatError(res);
    return res.json();
  }

  /**
   * Upload a file as attachment.
   *
   * Uses XMLHttpRequest for real upload progress events (fetch lacks upload progress).
   * Returns the Attachment metadata. `onProgress(percent)` is called 0-100.
   * `signal` (AbortSignal) allows cancellation.
   */
  uploadAttachment(
    channelId: string,
    file: File,
    opts?: {
      onProgress?: (percent: number) => void;
      signal?: AbortSignal;
    },
  ): Promise<Attachment> {
    return new Promise<Attachment>((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open(
        "POST",
        `${this.apiBase}/v1/channels/${channelId}/attachments`,
      );
      xhr.setRequestHeader("Authorization", `Bearer ${this.opts.token}`);

      xhr.upload.onprogress = (e) => {
        if (e.lengthComputable && opts?.onProgress) {
          opts.onProgress(Math.round((e.loaded / e.total) * 100));
        }
      };

      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          try {
            resolve(JSON.parse(xhr.responseText));
          } catch {
            reject(new ChatApiError(500, "Invalid response JSON"));
          }
        } else {
          reject(new ChatApiError(xhr.status, xhr.responseText || xhr.statusText));
        }
      };
      xhr.onerror = () => reject(new ChatApiError(0, "Network error"));
      xhr.onabort = () => reject(new ChatApiError(0, "Upload cancelled"));

      if (opts?.signal) {
        if (opts.signal.aborted) {
          xhr.abort();
          return;
        }
        opts.signal.addEventListener("abort", () => xhr.abort());
      }

      const formData = new FormData();
      formData.append("file", file, file.name);
      xhr.send(formData);
    });
  }

  /** Send typing indicator. */
  async sendTyping(channelId: string): Promise<void> {
    await fetch(`${this.apiBase}/v1/channels/${channelId}/typing`, {
      method: "POST",
      headers: this.headers,
    });
  }

  /** Mark a channel as read up to a message. */
  async markRead(channelId: string, messageId: string): Promise<void> {
    await fetch(`${this.apiBase}/v1/channels/${channelId}/ack`, {
      method: "POST",
      headers: this.headers,
      body: JSON.stringify({ message_id: messageId }),
    });
  }

  /** Bulk-fetch this user's read states for every channel they've touched. */
  async getReadStates(): Promise<ChannelReadState[]> {
    const res = await fetch(`${this.apiBase}/v1/read-states`, {
      headers: this.headers,
    });
    if (!res.ok) throw await chatError(res);
    return res.json();
  }

  /** Sync multiple channels after offline period. */
  async sync(channels: SyncChannelRequest[]): Promise<SyncResponse> {
    const res = await fetch(`${this.apiBase}/v1/sync`, {
      method: "POST",
      headers: this.headers,
      body: JSON.stringify({ channels }),
    });
    if (!res.ok) throw await chatError(res);
    return res.json();
  }
}

/** Structured error for API failures. */
export class ChatApiError extends Error {
  constructor(
    public status: number,
    public body: string,
    /**
     * Parsed `Retry-After` header (seconds). Set on 429 responses and any
     * other status that includes the header. The server emits it for rate
     * limits so the client can show an honest countdown rather than guess.
     */
    public retryAfter?: number,
  ) {
    super(`Chat API error ${status}: ${body}`);
    this.name = "ChatApiError";
  }
}

function parseRetryAfter(res: Response): number | undefined {
  const raw = res.headers.get("retry-after");
  if (!raw) return undefined;
  // Spec also allows HTTP-date; we only emit integer seconds, so don't
  // overengineer the parser. Bad value → undefined, caller picks a default.
  const n = parseInt(raw, 10);
  return Number.isFinite(n) && n > 0 ? n : undefined;
}

async function chatError(res: Response): Promise<ChatApiError> {
  return new ChatApiError(res.status, await res.text(), parseRetryAfter(res));
}
