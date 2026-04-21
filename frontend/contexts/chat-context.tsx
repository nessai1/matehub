import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useAuth } from "@/lib/auth";
import {
  ChatClient,
  type Attachment,
  type ChatClientEvent,
  type ConnectionState,
  type Message,
} from "@matehub/sdk-chat";

const CHAT_API = import.meta.env.VITE_CHAT_API_URL || "http://localhost:3003";

/// Sonyflake epoch (matches matehub-common::snowflake::SONYFLAKE_EPOCH_MS).
/// We use it to fabricate synthetic message_ids for optimistic sends so they
/// sort AFTER every real message in the current 10ms tick.
const SONYFLAKE_EPOCH_MS = 1_409_529_600_000;

function syntheticMessageId(counter: number): number {
  const tenMs = Math.floor((Date.now() - SONYFLAKE_EPOCH_MS) / 10);
  return tenMs * 16_777_216 + (counter & 0xffffff);
}

// ── Sound playback (module-level singleton) ───────

let audioCache: Record<string, HTMLAudioElement> = {};

function playSound(name: "message-in" | "message-out") {
  if (typeof window === "undefined") return;
  if (!audioCache[name]) {
    audioCache[name] = new Audio(`/sounds/${name}.ogg`);
    audioCache[name].volume = 0.4;
  }
  audioCache[name].currentTime = 0;
  audioCache[name].play().catch(() => {});
}

// ── Types ─────────────────────────────────────────

/**
 * UI-side augmentation of SDK `Message`. `_status` flags an optimistic message
 * that has not yet been confirmed by the server: `sending` while POST is in
 * flight, `failed` if it errored out. Server-confirmed messages leave the
 * field undefined so existing render paths don't have to branch on it.
 */
export type ChatMessage = Message & { _status?: "sending" | "failed" };

export interface ChannelUnread {
  lastReadMessageId: number;
  unread: number;
}

interface ChatContextValue {
  client: ChatClient | null;
  connectionState: ConnectionState;
  messagesByChannel: ReadonlyMap<number, ChatMessage[]>;
  typingByChannel: ReadonlyMap<number, string[]>;
  unreadByChannel: ReadonlyMap<number, ChannelUnread>;
  /** message_id snapshot at the instant the user entered a channel. The
   *  channel view draws a "New messages" divider above the first message
   *  whose id exceeds this. `null` = no unread at entry, no divider. */
  dividerByChannel: ReadonlyMap<number, number | null>;
  activeChannelId: number | null;
  setActiveChannelId: (id: number | null) => void;
  ensureHistory: (channelId: number) => void;
  loadMore: (channelId: number) => Promise<number>;
  sendMessage: (
    channelId: number,
    content: string,
    attachments?: Attachment[],
  ) => Promise<void>;
  sendTyping: (channelId: number) => void;
  /** Resend a previously-failed optimistic message. */
  retryMessage: (channelId: number, clientId: string) => void;
}

const ChatContext = createContext<ChatContextValue | null>(null);

export function ChatProvider({ children }: { children: ReactNode }) {
  const { session, handleUnauthorized } = useAuth();

  const clientRef = useRef<ChatClient | null>(null);
  const [client, setClient] = useState<ChatClient | null>(null);
  const [connectionState, setConnectionState] =
    useState<ConnectionState>("disconnected");

  const [messagesByChannel, setMessagesByChannel] = useState<
    Map<number, ChatMessage[]>
  >(() => new Map());
  const [typingByChannel, setTypingByChannel] = useState<Map<number, string[]>>(
    () => new Map(),
  );
  const [unreadByChannel, setUnreadByChannel] = useState<
    Map<number, ChannelUnread>
  >(() => new Map());
  const [dividerByChannel, setDividerByChannel] = useState<
    Map<number, number | null>
  >(() => new Map());

  const [activeChannelId, setActiveChannelIdState] = useState<number | null>(
    null,
  );
  const setActiveChannelId = useCallback((id: number | null) => {
    setActiveChannelIdState(id);
  }, []);

  // Ephemeral per-channel bookkeeping stored in refs.
  const typingTimers = useRef<
    Map<number, Map<string, ReturnType<typeof setTimeout>>>
  >(new Map());
  const lastMessageAt = useRef<Map<number, Map<string, number>>>(new Map());
  const loadingHistoryFor = useRef<Set<number>>(new Set());
  const loadedHistoryFor = useRef<Set<number>>(new Set());
  const synthCounter = useRef(0);
  /** Saved payloads for optimistic messages — replayed by `retryMessage`. */
  const pendingPayload = useRef<
    Map<
      string,
      { channelId: number; content: string; attachments?: Attachment[] }
    >
  >(new Map());

  // Mirrors of selected state into refs so event handlers can read the latest
  // values without re-binding on every render.
  const myUserIdRef = useRef<number | null>(null);
  useEffect(() => {
    myUserIdRef.current = session?.userId ?? null;
  }, [session?.userId]);

  const activeChannelRef = useRef<number | null>(null);
  useEffect(() => {
    activeChannelRef.current = activeChannelId;
  }, [activeChannelId]);

  const unreadByChannelRef = useRef<Map<number, ChannelUnread>>(new Map());
  useEffect(() => {
    unreadByChannelRef.current = unreadByChannel;
  }, [unreadByChannel]);

  const messagesByChannelRef = useRef<Map<number, ChatMessage[]>>(new Map());
  useEffect(() => {
    messagesByChannelRef.current = messagesByChannel;
  }, [messagesByChannel]);

  // ── Unread / ACK helpers ──────────────────────────

  /** Fire-and-forget ACK that also advances our local `lastReadMessageId`
   *  and zeroes the unread count. No-op when the local state is already
   *  past this message. */
  const ackUpTo = useCallback((channelId: number, messageId: number) => {
    const c = clientRef.current;
    if (!c) return;
    const prev = unreadByChannelRef.current.get(channelId);
    if (prev && prev.lastReadMessageId >= messageId && prev.unread === 0) {
      return;
    }
    c.markRead(channelId, messageId).catch((err) => {
      console.error("[ChatProvider] markRead failed:", err);
    });
    setUnreadByChannel((prevMap) => {
      const prevRow = prevMap.get(channelId);
      const next = new Map(prevMap);
      next.set(channelId, {
        lastReadMessageId: Math.max(prevRow?.lastReadMessageId ?? 0, messageId),
        unread: 0,
      });
      return next;
    });
  }, []);

  // ── WS lifecycle ──────────────────────────────────

  useEffect(() => {
    if (!session?.token || session.hubId == null) return;

    const c = new ChatClient({
      baseUrl: CHAT_API,
      token: session.token,
      hubId: session.hubId,
    });
    clientRef.current = c;
    setClient(c);

    const unsub = c.on((event: ChatClientEvent) => {
      switch (event.type) {
        case "connection.state":
          setConnectionState(event.state);
          break;

        case "message.new": {
          const msg = event.message;
          const chId = msg.channel_id;
          const myId = myUserIdRef.current;
          const isMine = myId != null && msg.author_id === String(myId);

          setMessagesByChannel((prev) => {
            const list = prev.get(chId);
            // If history not yet loaded for this channel, drop the event —
            // getHistory will backfill it soon. Avoids a single-message flash
            // of context before the full history renders.
            if (list === undefined) return prev;

            // Optimistic dedup: if we're already showing a synthetic message
            // with the same client_id, replace it with the real one. Keeps
            // the index stable so React reconciler doesn't unmount/remount.
            if (msg.client_id) {
              const idx = list.findIndex((m) => m.client_id === msg.client_id);
              if (idx !== -1) {
                const copy = [...list];
                copy[idx] = msg;
                const next = new Map(prev);
                next.set(chId, copy);
                return next;
              }
            }
            const next = new Map(prev);
            next.set(chId, [...list, msg]);
            return next;
          });
          if (msg.client_id) pendingPayload.current.delete(msg.client_id);

          // Clear author's typing indicator / remember when they last spoke.
          const authorId = msg.author_id;
          const chTimers = typingTimers.current.get(chId);
          if (chTimers?.has(authorId)) {
            clearTimeout(chTimers.get(authorId)!);
            chTimers.delete(authorId);
          }
          const chLast = lastMessageAt.current.get(chId) ?? new Map();
          chLast.set(authorId, Date.now());
          lastMessageAt.current.set(chId, chLast);
          setTypingByChannel((prev) => {
            const existing = prev.get(chId);
            if (!existing || !existing.includes(authorId)) return prev;
            const next = new Map(prev);
            next.set(chId, existing.filter((u) => u !== authorId));
            return next;
          });

          // Unread bookkeeping.
          const isFocused = activeChannelRef.current === chId;
          if (isMine) {
            // Advance lastRead silently; no server ACK (server already knows).
            setUnreadByChannel((prev) => {
              const prevRow = prev.get(chId);
              const next = new Map(prev);
              next.set(chId, {
                lastReadMessageId: Math.max(
                  prevRow?.lastReadMessageId ?? 0,
                  msg.message_id,
                ),
                unread: 0,
              });
              return next;
            });
          } else if (isFocused) {
            // User is watching this channel — ack on every new message so
            // the badge never lights up for the channel that's on screen.
            ackUpTo(chId, msg.message_id);
          } else {
            setUnreadByChannel((prev) => {
              const prevRow = prev.get(chId) ?? {
                lastReadMessageId: 0,
                unread: 0,
              };
              const next = new Map(prev);
              next.set(chId, {
                lastReadMessageId: prevRow.lastReadMessageId,
                unread: prevRow.unread + 1,
              });
              return next;
            });
          }

          if (isMine) playSound("message-out");
          else if (isFocused) playSound("message-in");
          break;
        }

        case "message.updated": {
          const { channel_id: chId, message_id, content } = event.data;
          setMessagesByChannel((prev) => {
            const list = prev.get(chId);
            if (!list) return prev;
            const next = new Map(prev);
            next.set(
              chId,
              list.map((m) =>
                m.message_id === message_id
                  ? { ...m, content, edited_at: "now" }
                  : m,
              ),
            );
            return next;
          });
          break;
        }

        case "message.deleted": {
          const { channel_id: chId, message_id } = event.data;
          setMessagesByChannel((prev) => {
            const list = prev.get(chId);
            if (!list) return prev;
            const next = new Map(prev);
            next.set(chId, list.filter((m) => m.message_id !== message_id));
            return next;
          });
          break;
        }

        case "attachment.updated": {
          const { channel_id: chId, message_id, attachments } = event.data;
          setMessagesByChannel((prev) => {
            const list = prev.get(chId);
            if (!list) return prev;
            const next = new Map(prev);
            next.set(
              chId,
              list.map((m) =>
                m.message_id === message_id ? { ...m, attachments } : m,
              ),
            );
            return next;
          });
          break;
        }

        case "typing.start": {
          const { channel_id: chId, user_id } = event.data;
          const myId = myUserIdRef.current;
          if (myId != null && user_id === String(myId)) break;

          // Anti-stale: NATS can deliver typing AFTER the message-create that
          // the same user just published. Drop the typing in that window.
          const chLast = lastMessageAt.current.get(chId);
          const lastAt = chLast?.get(user_id) ?? 0;
          if (Date.now() - lastAt < 2000) break;

          setTypingByChannel((prev) => {
            const existing = prev.get(chId) ?? [];
            if (existing.includes(user_id)) return prev;
            const next = new Map(prev);
            next.set(chId, [...existing, user_id]);
            return next;
          });

          let chTimers = typingTimers.current.get(chId);
          if (!chTimers) {
            chTimers = new Map();
            typingTimers.current.set(chId, chTimers);
          }
          if (chTimers.has(user_id)) clearTimeout(chTimers.get(user_id)!);
          chTimers.set(
            user_id,
            setTimeout(() => {
              setTypingByChannel((prev) => {
                const existing = prev.get(chId);
                if (!existing) return prev;
                const next = new Map(prev);
                next.set(chId, existing.filter((u) => u !== user_id));
                return next;
              });
              chTimers!.delete(user_id);
            }, 6000),
          );
          break;
        }

        case "error":
          console.error("[ChatProvider]", event.message);
          break;
      }
    });

    c.connect();

    // Seed unread map with the server-side read-state snapshot, then backfill
    // per-channel unread counts via /v1/sync. NATS publishes happen AFTER the
    // Scylla write, so by the time the sync Scylla-reads run, all previously
    // persisted messages are visible. Live MESSAGE_CREATE events arriving
    // during the sync window race with the overwrite; we accept a ±1
    // undercount in that narrow window — bounded error, no drift.
    (async () => {
      let states: Awaited<ReturnType<typeof c.getReadStates>>;
      try {
        states = await c.getReadStates();
      } catch (err) {
        console.error("[ChatProvider] getReadStates failed:", err);
        return;
      }

      setUnreadByChannel((prev) => {
        const next = new Map(prev);
        for (const s of states) {
          const row = next.get(s.channel_id);
          next.set(s.channel_id, {
            lastReadMessageId: Math.max(
              row?.lastReadMessageId ?? 0,
              s.last_read_message_id,
            ),
            unread: row?.unread ?? 0,
          });
        }
        return next;
      });

      // Count messages-since-last-read per channel. Sync caps at 50 channels
      // per request server-side; if a user legitimately has more, we batch.
      // `after: 0` channels (never read anything) are skipped — they haven't
      // been opened yet, so no "unread count" concept applies.
      const toSync = states.filter((s) => s.last_read_message_id > 0);
      if (toSync.length === 0) return;

      const myId = myUserIdRef.current;
      const myIdStr = myId != null ? String(myId) : null;
      const BATCH = 50;

      for (let i = 0; i < toSync.length; i += BATCH) {
        const slice = toSync.slice(i, i + BATCH);
        let resp: Awaited<ReturnType<typeof c.sync>>;
        try {
          resp = await c.sync(
            slice.map((s) => ({
              channel_id: s.channel_id,
              after: s.last_read_message_id,
            })),
          );
        } catch (err) {
          console.error("[ChatProvider] sync for unread failed:", err);
          return;
        }

        setUnreadByChannel((prev) => {
          const next = new Map(prev);
          for (const ch of resp.channels) {
            const row = next.get(ch.channel_id);
            if (!row) continue;
            const foreignCount = myIdStr
              ? ch.messages.filter((m) => m.author_id !== myIdStr).length
              : ch.messages.length;
            // When the server truncated (>300 events), `limited` is set and
            // `messages` is only the last 50. Clamp display to a sentinel so
            // the sidebar shows "99+" regardless of the real number.
            const unread = ch.limited ? Math.max(300, foreignCount) : foreignCount;
            // Merge with whatever is already there (the active channel's
            // MESSAGE_CREATE handler may have incremented meanwhile — keep
            // the max so we don't undercount).
            next.set(ch.channel_id, {
              lastReadMessageId: row.lastReadMessageId,
              unread: Math.max(row.unread, unread),
            });
          }
          return next;
        });
      }
    })();

    return () => {
      unsub();
      c.disconnect();
      clientRef.current = null;
      setClient(null);
      setMessagesByChannel(new Map());
      setTypingByChannel(new Map());
      setUnreadByChannel(new Map());
      setDividerByChannel(new Map());
      loadingHistoryFor.current.clear();
      loadedHistoryFor.current.clear();
      pendingPayload.current.clear();
      for (const chTimers of typingTimers.current.values()) {
        for (const t of chTimers.values()) clearTimeout(t);
      }
      typingTimers.current.clear();
      lastMessageAt.current.clear();
    };
  }, [session?.token, session?.hubId, ackUpTo]);

  // Propagate token refresh into the live client — no reconnect.
  useEffect(() => {
    if (session?.token && clientRef.current) {
      clientRef.current.updateToken(session.token);
    }
  }, [session?.token]);

  // ── Active-channel transitions: divider snapshot + auto-ack ──

  useEffect(() => {
    if (activeChannelId == null) return;
    const chId = activeChannelId;

    // (1) Divider snapshot — always overwrite on entry so a second visit
    //     reflects the current lastRead.
    setDividerByChannel((prev) => {
      const row = unreadByChannelRef.current.get(chId);
      const pos = row && row.unread > 0 ? row.lastReadMessageId : null;
      const next = new Map(prev);
      next.set(chId, pos);
      return next;
    });

    // (2) If history is already cached, ack up to the newest message. If not
    //     cached, ensureHistory's completion path handles the ack (see below).
    const list = messagesByChannelRef.current.get(chId);
    if (list && list.length > 0) {
      const newest = list[list.length - 1];
      const row = unreadByChannelRef.current.get(chId);
      if (!row || row.lastReadMessageId < newest.message_id) {
        ackUpTo(chId, newest.message_id);
      } else if (row.unread > 0) {
        // lastRead at newest already but the counter lagged — fix silently.
        setUnreadByChannel((prev) => {
          const next = new Map(prev);
          next.set(chId, { lastReadMessageId: row.lastReadMessageId, unread: 0 });
          return next;
        });
      }
    }
  }, [activeChannelId, ackUpTo]);

  // ── Imperative actions ────────────────────────────

  const ensureHistory = useCallback(
    (channelId: number) => {
      const c = clientRef.current;
      if (!c) return;
      if (loadedHistoryFor.current.has(channelId)) return;
      if (loadingHistoryFor.current.has(channelId)) return;
      loadingHistoryFor.current.add(channelId);

      c.getHistory(channelId, { limit: 50 })
        .then((history) => {
          loadedHistoryFor.current.add(channelId);
          loadingHistoryFor.current.delete(channelId);
          const reversed = history.reverse();

          setMessagesByChannel((prev) => {
            const existing = prev.get(channelId) ?? [];
            const histIds = new Set(reversed.map((m) => m.message_id));
            // Preserve optimistic messages that haven't been confirmed yet.
            const pending = existing.filter(
              (m) => m._status && !histIds.has(m.message_id),
            );
            const next = new Map(prev);
            next.set(channelId, [...reversed, ...pending]);
            return next;
          });

          // If this channel is currently active and we haven't acked the
          // freshly-loaded newest message, do it now.
          if (
            activeChannelRef.current === channelId &&
            reversed.length > 0
          ) {
            const newest = reversed[reversed.length - 1];
            const row = unreadByChannelRef.current.get(channelId);
            if (!row || row.lastReadMessageId < newest.message_id) {
              ackUpTo(channelId, newest.message_id);
            }
          }
        })
        .catch((err) => {
          loadingHistoryFor.current.delete(channelId);
          if (
            err &&
            typeof err === "object" &&
            "status" in err &&
            (err as { status: number }).status === 401
          ) {
            handleUnauthorized();
            return;
          }
          console.error("[ChatProvider] history error:", err);
        });
    },
    [handleUnauthorized, ackUpTo],
  );

  const loadMore = useCallback(
    async (channelId: number): Promise<number> => {
      const c = clientRef.current;
      if (!c) return 0;
      const existing = messagesByChannelRef.current.get(channelId) ?? [];
      if (existing.length === 0) return 0;
      const oldest = existing[0];
      const older = await c.getHistory(channelId, {
        limit: 50,
        before: oldest.message_id,
      });
      if (older.length === 0) return 0;
      setMessagesByChannel((prev) => {
        const list = prev.get(channelId) ?? [];
        const next = new Map(prev);
        next.set(channelId, [...older.reverse(), ...list]);
        return next;
      });
      return older.length;
    },
    [],
  );

  const doSend = useCallback(
    async (
      channelId: number,
      clientId: string,
      content: string,
      attachments?: Attachment[],
    ) => {
      const c = clientRef.current;
      if (!c) return;
      try {
        await c.sendMessage(channelId, { content, clientId, attachments });
        // Success path: the real MESSAGE_CREATE event will replace the
        // synthetic by client_id. Nothing to do here.
      } catch (err: unknown) {
        if (
          err &&
          typeof err === "object" &&
          "status" in err &&
          (err as { status: number }).status === 401
        ) {
          handleUnauthorized();
          return;
        }
        setMessagesByChannel((prev) => {
          const list = prev.get(channelId);
          if (!list) return prev;
          const idx = list.findIndex((m) => m.client_id === clientId);
          if (idx === -1) return prev;
          const copy = [...list];
          copy[idx] = { ...copy[idx], _status: "failed" };
          const next = new Map(prev);
          next.set(channelId, copy);
          return next;
        });
        console.error("[ChatProvider] sendMessage failed:", err);
      }
    },
    [handleUnauthorized],
  );

  const sendMessage = useCallback(
    async (
      channelId: number,
      content: string,
      attachments?: Attachment[],
    ) => {
      const c = clientRef.current;
      const myId = myUserIdRef.current;
      if (!c || myId == null) return;
      const trimmed = content.trim();
      const hasContent = trimmed.length > 0;
      const hasAttachments = attachments && attachments.length > 0;
      if (!hasContent && !hasAttachments) return;

      const clientId =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `${Date.now()}-${Math.random().toString(36).slice(2)}`;

      synthCounter.current = (synthCounter.current + 1) & 0xffffff;
      const synthetic: ChatMessage = {
        hub_id: session?.hubId ?? 0,
        channel_id: channelId,
        message_id: syntheticMessageId(synthCounter.current),
        author_id: String(myId),
        author_type: "permanent",
        content: trimmed,
        thread_root_id: null,
        mentions: [],
        mention_groups: [],
        mention_everyone: false,
        attachments: attachments ?? [],
        edited_at: null,
        deleted_at: null,
        client_id: clientId,
        bucket: 0,
        _status: "sending",
      };

      pendingPayload.current.set(clientId, {
        channelId,
        content: trimmed,
        attachments,
      });

      setMessagesByChannel((prev) => {
        const list = prev.get(channelId) ?? [];
        const next = new Map(prev);
        next.set(channelId, [...list, synthetic]);
        return next;
      });

      await doSend(channelId, clientId, trimmed, attachments);
    },
    [session?.hubId, doSend],
  );

  const retryMessage = useCallback(
    (channelId: number, clientId: string) => {
      const payload = pendingPayload.current.get(clientId);
      if (!payload) return;
      setMessagesByChannel((prev) => {
        const list = prev.get(channelId);
        if (!list) return prev;
        const idx = list.findIndex((m) => m.client_id === clientId);
        if (idx === -1) return prev;
        const copy = [...list];
        copy[idx] = { ...copy[idx], _status: "sending" };
        const next = new Map(prev);
        next.set(channelId, copy);
        return next;
      });
      void doSend(channelId, clientId, payload.content, payload.attachments);
    },
    [doSend],
  );

  const sendTyping = useCallback((channelId: number) => {
    clientRef.current?.sendTyping(channelId);
  }, []);

  const value = useMemo<ChatContextValue>(
    () => ({
      client,
      connectionState,
      messagesByChannel,
      typingByChannel,
      unreadByChannel,
      dividerByChannel,
      activeChannelId,
      setActiveChannelId,
      ensureHistory,
      loadMore,
      sendMessage,
      sendTyping,
      retryMessage,
    }),
    [
      client,
      connectionState,
      messagesByChannel,
      typingByChannel,
      unreadByChannel,
      dividerByChannel,
      activeChannelId,
      setActiveChannelId,
      ensureHistory,
      loadMore,
      sendMessage,
      sendTyping,
      retryMessage,
    ],
  );

  return <ChatContext.Provider value={value}>{children}</ChatContext.Provider>;
}

export function useChatContext(): ChatContextValue {
  const ctx = useContext(ChatContext);
  if (!ctx) {
    throw new Error("useChatContext must be used within <ChatProvider>");
  }
  return ctx;
}

/**
 * Per-channel selector. Matches the old `useChatClient` shape so call sites
 * don't need to change; adds `dividerPos` and `retryMessage` for the new
 * unread-divider and optimistic-retry flows.
 */
export function useChatClient(channelId: number) {
  const ctx = useChatContext();

  useEffect(() => {
    ctx.ensureHistory(channelId);
  }, [channelId, ctx]);

  useEffect(() => {
    ctx.setActiveChannelId(channelId);
  }, [channelId, ctx]);

  const messages = ctx.messagesByChannel.get(channelId) ?? [];
  const typingUsers = ctx.typingByChannel.get(channelId) ?? [];
  const dividerPos = ctx.dividerByChannel.get(channelId) ?? null;

  const sendMessage = useCallback(
    (content: string, attachments?: Attachment[]) =>
      ctx.sendMessage(channelId, content, attachments),
    [ctx, channelId],
  );

  const sendTyping = useCallback(
    () => ctx.sendTyping(channelId),
    [ctx, channelId],
  );

  const loadMore = useCallback(
    () => ctx.loadMore(channelId),
    [ctx, channelId],
  );

  const retryMessage = useCallback(
    (clientId: string) => ctx.retryMessage(channelId, clientId),
    [ctx, channelId],
  );

  return {
    client: ctx.client,
    messages,
    connectionState: ctx.connectionState,
    typingUsers,
    dividerPos,
    sendMessage,
    sendTyping,
    loadMore,
    retryMessage,
  };
}

/** Sidebar-facing subscription to per-channel unread counts. */
export function useUnreadCounts(): ReadonlyMap<number, ChannelUnread> {
  return useChatContext().unreadByChannel;
}
