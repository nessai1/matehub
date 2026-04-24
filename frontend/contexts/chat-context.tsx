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
const SONYFLAKE_EPOCH_MS = 1_409_529_600_000;

/**
 * Fabricate a `message_id` for an optimistic send. Returns a decimal string
 * larger than any real Sonyflake from the same 10 ms tick, so synthetic
 * messages sort to the end of the list until the server echo replaces them.
 */
function syntheticMessageId(counter: number): string {
  const tenMs = Math.floor((Date.now() - SONYFLAKE_EPOCH_MS) / 10);
  const id = BigInt(tenMs) * 16_777_216n + BigInt(counter & 0xffffff);
  return id.toString();
}

/**
 * Compare two decimal-string Snowflake ids. Returns <0 / 0 / >0 like a
 * classic comparator. Uses BigInt because string lexicographic compare
 * breaks across digit-count boundaries (e.g. "9" > "10").
 */
function cmpIds(a: string, b: string): number {
  const ab = BigInt(a);
  const bb = BigInt(b);
  if (ab < bb) return -1;
  if (ab > bb) return 1;
  return 0;
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
 * flight, `failed` if it errored out.
 */
export type ChatMessage = Message & { _status?: "sending" | "failed" };

export interface ChannelUnread {
  lastReadMessageId: string;
  unread: number;
}

interface ChatContextValue {
  client: ChatClient | null;
  connectionState: ConnectionState;
  messagesByChannel: ReadonlyMap<string, ChatMessage[]>;
  typingByChannel: ReadonlyMap<string, string[]>;
  unreadByChannel: ReadonlyMap<string, ChannelUnread>;
  /** message_id snapshot at the instant the user entered a channel. */
  dividerByChannel: ReadonlyMap<string, string | null>;
  activeChannelId: string | null;
  setActiveChannelId: (id: string | null) => void;
  ensureHistory: (channelId: string) => void;
  loadMore: (channelId: string) => Promise<number>;
  sendMessage: (
    channelId: string,
    content: string,
    attachments?: Attachment[],
  ) => Promise<void>;
  sendTyping: (channelId: string) => void;
  retryMessage: (channelId: string, clientId: string) => void;
}

const ChatContext = createContext<ChatContextValue | null>(null);

export function ChatProvider({ children }: { children: ReactNode }) {
  const { session, handleUnauthorized } = useAuth();

  const clientRef = useRef<ChatClient | null>(null);
  const [client, setClient] = useState<ChatClient | null>(null);
  const [connectionState, setConnectionState] =
    useState<ConnectionState>("disconnected");

  const [messagesByChannel, setMessagesByChannel] = useState<
    Map<string, ChatMessage[]>
  >(() => new Map());
  const [typingByChannel, setTypingByChannel] = useState<Map<string, string[]>>(
    () => new Map(),
  );
  const [unreadByChannel, setUnreadByChannel] = useState<
    Map<string, ChannelUnread>
  >(() => new Map());
  const [dividerByChannel, setDividerByChannel] = useState<
    Map<string, string | null>
  >(() => new Map());

  const [activeChannelId, setActiveChannelIdState] = useState<string | null>(
    null,
  );
  const setActiveChannelId = useCallback((id: string | null) => {
    setActiveChannelIdState(id);
  }, []);

  const typingTimers = useRef<
    Map<string, Map<string, ReturnType<typeof setTimeout>>>
  >(new Map());
  const lastMessageAt = useRef<Map<string, Map<string, number>>>(new Map());
  const loadingHistoryFor = useRef<Set<string>>(new Set());
  const loadedHistoryFor = useRef<Set<string>>(new Set());
  const synthCounter = useRef(0);
  const pendingPayload = useRef<
    Map<
      string,
      { channelId: string; content: string; attachments?: Attachment[] }
    >
  >(new Map());

  const myUserIdRef = useRef<string | null>(null);
  useEffect(() => {
    myUserIdRef.current = session?.userId ?? null;
  }, [session?.userId]);

  const activeChannelRef = useRef<string | null>(null);
  useEffect(() => {
    activeChannelRef.current = activeChannelId;
  }, [activeChannelId]);

  const unreadByChannelRef = useRef<Map<string, ChannelUnread>>(new Map());
  useEffect(() => {
    unreadByChannelRef.current = unreadByChannel;
  }, [unreadByChannel]);

  const messagesByChannelRef = useRef<Map<string, ChatMessage[]>>(new Map());
  useEffect(() => {
    messagesByChannelRef.current = messagesByChannel;
  }, [messagesByChannel]);

  // ── Unread / ACK helpers ──────────────────────────

  const ackUpTo = useCallback((channelId: string, messageId: string) => {
    const c = clientRef.current;
    if (!c) return;
    const prev = unreadByChannelRef.current.get(channelId);
    if (
      prev &&
      cmpIds(prev.lastReadMessageId, messageId) >= 0 &&
      prev.unread === 0
    ) {
      return;
    }
    c.markRead(channelId, messageId).catch((err) => {
      console.error("[ChatProvider] markRead failed:", err);
    });
    setUnreadByChannel((prevMap) => {
      const prevRow = prevMap.get(channelId);
      const next = new Map(prevMap);
      const lastRead =
        prevRow && cmpIds(prevRow.lastReadMessageId, messageId) > 0
          ? prevRow.lastReadMessageId
          : messageId;
      next.set(channelId, {
        lastReadMessageId: lastRead,
        unread: 0,
      });
      return next;
    });
  }, []);

  // ── WS lifecycle ──────────────────────────────────

  useEffect(() => {
    if (!session?.token || !session.hubId) return;

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
          const isMine = myId != null && msg.author_id === myId;

          setMessagesByChannel((prev) => {
            const list = prev.get(chId);
            if (list === undefined) return prev;
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

          const isFocused = activeChannelRef.current === chId;
          if (isMine) {
            setUnreadByChannel((prev) => {
              const prevRow = prev.get(chId);
              const next = new Map(prev);
              const lastRead =
                prevRow &&
                cmpIds(prevRow.lastReadMessageId, msg.message_id) > 0
                  ? prevRow.lastReadMessageId
                  : msg.message_id;
              next.set(chId, { lastReadMessageId: lastRead, unread: 0 });
              return next;
            });
          } else if (isFocused) {
            ackUpTo(chId, msg.message_id);
          } else {
            setUnreadByChannel((prev) => {
              const prevRow = prev.get(chId) ?? {
                lastReadMessageId: "0",
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
          if (myId != null && user_id === myId) break;

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
          const lastRead =
            row && cmpIds(row.lastReadMessageId, s.last_read_message_id) > 0
              ? row.lastReadMessageId
              : s.last_read_message_id;
          next.set(s.channel_id, {
            lastReadMessageId: lastRead,
            unread: row?.unread ?? 0,
          });
        }
        return next;
      });

      // "0" sentinel means "never read anything" — skip backfill.
      const toSync = states.filter((s) => s.last_read_message_id !== "0");
      if (toSync.length === 0) return;

      const myId = myUserIdRef.current;
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
            const foreignCount = myId
              ? ch.messages.filter((m) => m.author_id !== myId).length
              : ch.messages.length;
            const unread = ch.limited ? Math.max(300, foreignCount) : foreignCount;
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

  useEffect(() => {
    if (session?.token && clientRef.current) {
      clientRef.current.updateToken(session.token);
    }
  }, [session?.token]);

  // ── Active-channel transitions: divider snapshot + auto-ack ──

  useEffect(() => {
    if (activeChannelId == null) return;
    const chId = activeChannelId;

    setDividerByChannel((prev) => {
      const row = unreadByChannelRef.current.get(chId);
      const pos = row && row.unread > 0 ? row.lastReadMessageId : null;
      const next = new Map(prev);
      next.set(chId, pos);
      return next;
    });

    const list = messagesByChannelRef.current.get(chId);
    if (list && list.length > 0) {
      const newest = list[list.length - 1];
      const row = unreadByChannelRef.current.get(chId);
      if (!row || cmpIds(row.lastReadMessageId, newest.message_id) < 0) {
        ackUpTo(chId, newest.message_id);
      } else if (row.unread > 0) {
        setUnreadByChannel((prev) => {
          const next = new Map(prev);
          next.set(chId, {
            lastReadMessageId: row.lastReadMessageId,
            unread: 0,
          });
          return next;
        });
      }
    }
  }, [activeChannelId, ackUpTo]);

  // ── Imperative actions ────────────────────────────

  const ensureHistory = useCallback(
    (channelId: string) => {
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
            const pending = existing.filter(
              (m) => m._status && !histIds.has(m.message_id),
            );
            const next = new Map(prev);
            next.set(channelId, [...reversed, ...pending]);
            return next;
          });

          if (
            activeChannelRef.current === channelId &&
            reversed.length > 0
          ) {
            const newest = reversed[reversed.length - 1];
            const row = unreadByChannelRef.current.get(channelId);
            if (
              !row ||
              cmpIds(row.lastReadMessageId, newest.message_id) < 0
            ) {
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
    async (channelId: string): Promise<number> => {
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
      channelId: string,
      clientId: string,
      content: string,
      attachments?: Attachment[],
    ) => {
      const c = clientRef.current;
      if (!c) return;
      try {
        await c.sendMessage(channelId, { content, clientId, attachments });
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
      channelId: string,
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
        hub_id: session?.hubId ?? "0",
        channel_id: channelId,
        message_id: syntheticMessageId(synthCounter.current),
        author_id: myId,
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
    (channelId: string, clientId: string) => {
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

  const sendTyping = useCallback((channelId: string) => {
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

export function useChatClient(channelId: string) {
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

export function useUnreadCounts(): ReadonlyMap<string, ChannelUnread> {
  return useChatContext().unreadByChannel;
}

/** Snowflake id comparator exposed for UI code that needs to sort by id. */
export { cmpIds };
