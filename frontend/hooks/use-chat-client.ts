"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useAuth, type AuthSession } from "@/lib/auth";
import {
  ChatClient,
  type ChatClientEvent,
  type ConnectionState,
  type Message,
  type Attachment,
} from "@matehub/sdk-chat";

const CHAT_API = process.env.NEXT_PUBLIC_CHAT_API_URL || "http://localhost:3003";


// ── Sound playback ───────────────────────────────

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

// ── Hook ─────────────────────────────────────────

export function useChatClient(channelId: string) {
  const { session, handleUnauthorized } = useAuth();
  const clientRef = useRef<ChatClient | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [connectionState, setConnectionState] = useState<ConnectionState>("disconnected");
  const [typingUsers, setTypingUsers] = useState<string[]>([]);
  const typingTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  /** Track when each user last sent a message -- ignores stale typing events */
  const lastMessageAt = useRef<Record<string, number>>({});
  const myUserId = session?.userId ?? "";
  // i64 hash of channelId -- resolved from first history/message response
  const channelHash = useRef<number | null>(null);

  // ── Lifecycle: create client, connect, cleanup ──

  useEffect(() => {
    if (!session?.token) return;

    const client = new ChatClient({
      baseUrl: CHAT_API,
      token: session.token,
      hubId: session.hubId,
    });
    clientRef.current = client;

    const unsub = client.on((event: ChatClientEvent) => {
      switch (event.type) {
        case "connection.state":
          setConnectionState(event.state);
          break;

        case "message.new":
          if (channelHash.current !== null && event.message.channel_id === channelHash.current) {
            setMessages((prev) => [...prev, event.message]);
            // Author stopped typing since they just sent
            const authorId = event.message.author_id;
            lastMessageAt.current[authorId] = Date.now();
            setTypingUsers((prev) => prev.filter((u) => u !== authorId));
            if (typingTimers.current[authorId]) {
              clearTimeout(typingTimers.current[authorId]);
              delete typingTimers.current[authorId];
            }
            // Sound: incoming if from someone else, outgoing if mine
            if (authorId === myUserId) {
              playSound("message-out");
            } else {
              playSound("message-in");
            }
          }
          break;

        case "message.updated":
          if (channelHash.current !== null && event.data.channel_id === channelHash.current) {
            setMessages((prev) =>
              prev.map((m) =>
                m.message_id === event.data.message_id
                  ? { ...m, content: event.data.content, edited_at: "now" }
                  : m,
              ),
            );
          }
          break;

        case "message.deleted":
          if (channelHash.current !== null && event.data.channel_id === channelHash.current) {
            setMessages((prev) =>
              prev.filter((m) => m.message_id !== event.data.message_id),
            );
          }
          break;

        case "attachment.updated":
          if (channelHash.current !== null && event.data.channel_id === channelHash.current) {
            setMessages((prev) =>
              prev.map((m) =>
                m.message_id === event.data.message_id
                  ? { ...m, attachments: event.data.attachments }
                  : m,
              ),
            );
          }
          break;

        case "typing.start":
          if (
            channelHash.current !== null && event.data.channel_id === channelHash.current &&
            event.data.user_id !== myUserId
          ) {
            const uid = event.data.user_id;
            // Ignore stale typing arriving right after a message from same user (NATS ordering)
            const lastMsg = lastMessageAt.current[uid] ?? 0;
            if (Date.now() - lastMsg < 2000) break;
            setTypingUsers((prev) =>
              prev.includes(uid) ? prev : [...prev, uid],
            );
            // Clear after 6s
            if (typingTimers.current[uid]) {
              clearTimeout(typingTimers.current[uid]);
            }
            typingTimers.current[uid] = setTimeout(() => {
              setTypingUsers((prev) => prev.filter((u) => u !== uid));
              delete typingTimers.current[uid];
            }, 6000);
          }
          break;

        case "error":
          console.error("[useChatClient]", event.message);
          break;
      }
    });

    client.connect();

    // Load history
    client
      .getHistory(channelId, { limit: 50 })
      .then((history) => {
        // History comes newest-first, reverse for chronological
        setMessages(history.reverse());
        // Cache i64 hash from first message for WS event filtering
        if (history.length > 0) {
          channelHash.current = history[0].channel_id;
        }
      })
      .catch((err) => {
        if (err && typeof err === "object" && "status" in err && (err as { status: number }).status === 401) {
          handleUnauthorized();
          return;
        }
        console.error("[useChatClient] history error:", err);
      });

    return () => {
      unsub();
      client.disconnect();
      clientRef.current = null;
      channelHash.current = null;
      setMessages([]);
      setTypingUsers([]);
      // Clear typing timers
      for (const t of Object.values(typingTimers.current)) clearTimeout(t);
      typingTimers.current = {};
    };
  }, [session?.token, session?.hubId, channelId, myUserId]);

  // Update token on refresh
  useEffect(() => {
    if (session?.token && clientRef.current) {
      clientRef.current.updateToken(session.token);
    }
  }, [session?.token]);

  // ── Actions ────────────────────────────────────

  const sendMessage = useCallback(
    async (content: string, attachments?: Attachment[]) => {
      if (!clientRef.current) return;
      const hasContent = content.trim().length > 0;
      const hasAttachments = attachments && attachments.length > 0;
      if (!hasContent && !hasAttachments) return;
      try {
        const msg = await clientRef.current.sendMessage(channelId, {
          content: content.trim(),
          attachments,
        });
        if (!channelHash.current && msg) {
          channelHash.current = msg.channel_id;
        }
      } catch (err: unknown) {
        if (err && typeof err === "object" && "status" in err && (err as { status: number }).status === 401) {
          handleUnauthorized();
          return;
        }
        throw err;
      }
    },
    [channelId, handleUnauthorized],
  );

  const sendTyping = useCallback(() => {
    clientRef.current?.sendTyping(channelId);
  }, [channelId]);

  const loadMore = useCallback(async () => {
    if (!clientRef.current || messages.length === 0) return;
    const oldest = messages[0];
    const older = await clientRef.current.getHistory(channelId, {
      limit: 50,
      before: oldest.message_id,
    });
    if (older.length > 0) {
      setMessages((prev) => [...older.reverse(), ...prev]);
    }
    return older.length;
  }, [channelId, messages]);

  return {
    client: clientRef.current,
    messages,
    connectionState,
    typingUsers,
    sendMessage,
    sendTyping,
    loadMore,
  };
}
