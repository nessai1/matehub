"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Hash,
  SendHorizontal,
  Loader2,
  PlusIcon,
  SmileIcon,
  AtSignIcon,
  PaperclipIcon,
  MicIcon,
  BoldIcon,
  ItalicIcon,
  UnderlineIcon,
  LinkIcon,
  ListIcon,
  ListOrderedIcon,
  CodeIcon,
} from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { MemberCard } from "@/components/hub/member-card";
import { useChatClient } from "@/hooks/use-chat-client";
import { useMembers, type Member, type MemberGroup } from "@/hooks/use-members";
import { useAuth } from "@/lib/auth";
import { cn } from "@/lib/utils";
import type { Message } from "@matehub/sdk-chat";

interface TextChannelViewProps {
  channelId: string;
  channelName: string;
}

// ── Sonyflake timestamp extraction ───────────────
// Layout: time(39 bits, 10ms units) | sequence(8) | machine(16)
// Epoch: 2014-09-01T00:00:00Z

const SONYFLAKE_EPOCH_MS = 1409529600000;

function snowflakeToDate(id: number): Date {
  const time10ms = Math.floor(id / 16777216); // >> 24 bits
  return new Date(SONYFLAKE_EPOCH_MS + time10ms * 10);
}

function formatTime(messageId: number): string {
  return snowflakeToDate(messageId).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}

// ── Message bubble ───────────────────────────────

function ChatMessage({
  message,
  member,
  isGrouped,
  allGroups,
  onGroupsChanged,
}: {
  message: Message;
  member?: Member;
  isGrouped: boolean;
  allGroups: MemberGroup[];
  onGroupsChanged: () => void;
}) {
  const displayName = member?.display_name || member?.username || message.author_id;
  const initials = displayName.slice(0, 2).toUpperCase();
  const avatarUrl = member?.avatar_url;
  const roleColor = member?.groups?.[0]?.color;

  const avatarEl = (
    <button className="mt-0.5 shrink-0 cursor-pointer">
      <Avatar>
        {avatarUrl && <AvatarImage src={avatarUrl} alt={displayName} />}
        <AvatarFallback>{initials}</AvatarFallback>
      </Avatar>
    </button>
  );

  const nameEl = (
    <button
      className="cursor-pointer text-sm font-medium hover:underline"
      style={roleColor ? { color: roleColor } : undefined}
    >
      {displayName}
    </button>
  );

  // Wrap avatar + name in MemberCard popover if member is known
  const wrapWithCard = (el: React.ReactNode) =>
    member ? (
      <MemberCard member={member} allGroups={allGroups} onGroupsChanged={onGroupsChanged}>
        {el}
      </MemberCard>
    ) : (
      el
    );

  if (isGrouped) {
    return (
      <div className="group flex gap-3 py-0.5 pl-11 hover:bg-muted/30">
        <span className="invisible absolute -ml-8 pt-0.5 text-[10px] text-muted-foreground group-hover:visible">
          {formatTime(message.message_id)}
        </span>
        <div className="min-w-0 flex-1">
          <p className="text-sm leading-relaxed break-words">{message.content}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="group relative mt-3 flex gap-3 py-1 hover:bg-muted/30 first:mt-0">
      {wrapWithCard(avatarEl)}
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          {wrapWithCard(nameEl)}
          <span className="text-[10px] text-muted-foreground">
            {formatTime(message.message_id)}
          </span>
          {message.edited_at && (
            <span className="text-[10px] text-muted-foreground">(edited)</span>
          )}
        </div>
        <p className="text-sm leading-relaxed break-words">{message.content}</p>
      </div>
    </div>
  );
}

// ── Typing indicator ─────────────────────────────

function TypingIndicator({
  users,
  members,
}: {
  users: string[];
  members: Member[];
}) {
  if (users.length === 0) return null;

  const names = users.map((uid) => {
    const m = members.find((mm) => mm.user_id === uid);
    return m?.display_name || m?.username || uid;
  });

  let text: string;
  if (names.length === 1) {
    text = `${names[0]} is typing`;
  } else if (names.length === 2) {
    text = `${names[0]} and ${names[1]} are typing`;
  } else {
    text = `${names.length} people are typing`;
  }

  return (
    <div className="flex items-center gap-2 px-4 py-1 text-xs text-muted-foreground">
      <span className="flex gap-0.5">
        <span className="h-1 w-1 animate-bounce rounded-full bg-muted-foreground [animation-delay:0ms]" />
        <span className="h-1 w-1 animate-bounce rounded-full bg-muted-foreground [animation-delay:150ms]" />
        <span className="h-1 w-1 animate-bounce rounded-full bg-muted-foreground [animation-delay:300ms]" />
      </span>
      <span>{text}</span>
    </div>
  );
}

// ── Main component ───────────────────────────────

export function TextChannelView({ channelId, channelName }: TextChannelViewProps) {
  const { session } = useAuth();
  const { messages, connectionState, typingUsers, sendMessage, sendTyping, loadMore } =
    useChatClient(channelId);
  const { members, loading: membersLoading, refetch } = useMembers();
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const isAtBottom = useRef(true);
  const typingThrottle = useRef(0);

  // Member lookup map (useMemo so re-render happens when members load)
  const memberMap = useMemo(() => {
    const map = new Map<string, Member>();
    for (const m of members) map.set(m.user_id, m);
    return map;
  }, [members]);

  // Collect unique groups for MemberCard
  const allGroups = useMemo(() => {
    const seen = new Set<string>();
    const groups: MemberGroup[] = [];
    for (const m of members) {
      for (const g of m.groups) {
        if (!seen.has(g.id)) {
          seen.add(g.id);
          groups.push(g);
        }
      }
    }
    return groups;
  }, [members]);

  // ── Auto-scroll to bottom on new messages ──

  useEffect(() => {
    if (isAtBottom.current) {
      bottomRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [messages]);

  const handleScroll = useCallback((e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    isAtBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
  }, []);

  // ── Send ──

  const handleSend = useCallback(async () => {
    if (!input.trim() || sending) return;
    const text = input;
    setInput("");
    setSending(true);
    try {
      await sendMessage(text);
    } catch (err) {
      console.error("send failed:", err);
      setInput(text); // restore on failure
    } finally {
      setSending(false);
    }
  }, [input, sending, sendMessage]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend],
  );

  // ── Typing throttle (5s) ──

  const handleInput = useCallback(
    (value: string) => {
      setInput(value);
      const now = Date.now();
      if (now - typingThrottle.current > 5000) {
        typingThrottle.current = now;
        sendTyping();
      }
    },
    [sendTyping],
  );

  // ── Check if messages should be grouped (same author, <2min apart) ──

  function isGroupedWith(prev: Message | undefined, curr: Message): boolean {
    if (!prev) return false;
    return prev.author_id === curr.author_id;
  }

  const isConnected = connectionState === "connected";
  const isConnecting = connectionState === "connecting" || connectionState === "identifying" || connectionState === "resuming";

  return (
    <div className="flex flex-1 flex-col">
      {/* ── Header ── */}
      <header className="flex h-10 shrink-0 items-center gap-2 border-b px-4">
        <Hash className="h-4 w-4 text-muted-foreground" />
        <span className="text-sm font-medium">{channelName}</span>
        {!isConnected && (
          <span className="ml-auto flex items-center gap-1.5 text-xs text-muted-foreground">
            {isConnecting ? (
              <>
                <Loader2 className="h-3 w-3 animate-spin" />
                Connecting...
              </>
            ) : connectionState === "reconnecting" ? (
              <>
                <Loader2 className="h-3 w-3 animate-spin" />
                Reconnecting...
              </>
            ) : (
              <span className="text-destructive">Disconnected</span>
            )}
          </span>
        )}
      </header>

      {/* ── Message list ── */}
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto px-4"
        onScroll={handleScroll}
      >
        {messages.length === 0 && isConnected && (
          <div className="flex h-full items-center justify-center">
            <div className="text-center text-muted-foreground">
              <Hash className="mx-auto mb-2 h-10 w-10 opacity-20" />
              <p className="text-sm font-medium">
                Welcome to #{channelName}
              </p>
              <p className="mt-1 text-xs">
                This is the start of the channel.
              </p>
            </div>
          </div>
        )}

        {(membersLoading || (messages.length === 0 && !isConnected)) && (
          <div className="space-y-4 py-4">
            {Array.from({ length: 5 }).map((_, i) => (
              <div key={i} className="flex gap-3">
                <Skeleton className="h-8 w-8 rounded-md" />
                <div className="flex-1 space-y-1.5">
                  <Skeleton className="h-3 w-24" />
                  <Skeleton className="h-3 w-48" />
                </div>
              </div>
            ))}
          </div>
        )}

        {!membersLoading && (
          <div className="py-2">
            {messages.map((msg, i) => (
              <ChatMessage
                key={msg.message_id}
                message={msg}
                member={memberMap.get(msg.author_id)}
                isGrouped={isGroupedWith(messages[i - 1], msg)}
                allGroups={allGroups}
                onGroupsChanged={refetch}
              />
            ))}
          </div>
        )}
        <div ref={bottomRef} />
      </div>

      {/* ── Typing indicator ── */}
      <TypingIndicator users={typingUsers} members={members} />

      {/* ── Input ── */}
      <div className="px-4 pb-4">
        <div className="flex flex-col rounded-xl border border-border/50 bg-muted/20 transition-colors focus-within:border-border">
          {/* Drag handle to resize */}
          <div
            className="group flex cursor-row-resize items-center justify-center pt-1"
            onMouseDown={(e) => {
              e.preventDefault();
              const container = e.currentTarget.parentElement!;
              // Lock current height to prevent jump
              const startH = container.offsetHeight;
              container.style.height = startH + "px";
              const startY = e.clientY;
              const onMove = (ev: MouseEvent) => {
                const delta = startY - ev.clientY;
                const newH = Math.max(startH, Math.min(startH + delta, 400));
                container.style.height = newH + "px";
              };
              const onUp = () => {
                document.removeEventListener("mousemove", onMove);
                document.removeEventListener("mouseup", onUp);
              };
              document.addEventListener("mousemove", onMove);
              document.addEventListener("mouseup", onUp);
            }}
          >
            <div className="h-1 w-8 rounded-full bg-muted-foreground/20 transition-colors group-hover:bg-muted-foreground/40" />
          </div>

          {/* Top: formatting toolbar */}
          <div className="flex items-center gap-0.5 border-b border-border/30 px-2 py-1">
            {[BoldIcon, ItalicIcon, UnderlineIcon, LinkIcon, ListOrderedIcon, ListIcon, CodeIcon].map(
              (Icon, i) => (
                <button
                  key={i}
                  className="rounded p-1 text-muted-foreground/50 transition-colors hover:bg-muted/50 hover:text-muted-foreground"
                >
                  <Icon className="h-3.5 w-3.5" />
                </button>
              ),
            )}
          </div>

          {/* Middle: textarea (grows to fill when dragged) */}
          <div className="min-h-[4rem] flex-1 overflow-auto px-3 py-1">
            <textarea
              value={input}
              onChange={(e) => handleInput(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder={`Type something... # ${channelName}`}
              disabled={!isConnected}
              rows={1}
              className={cn(
                "h-full w-full resize-none bg-transparent text-sm outline-none",
                "placeholder:text-muted-foreground/40",
                "disabled:cursor-not-allowed disabled:opacity-50",
              )}
            />
          </div>

          {/* Bottom: action buttons + send */}
          <div className="flex items-center justify-between border-t border-border/30 px-2 py-1">
            <div className="flex items-center gap-0.5">
              {[PlusIcon, SmileIcon, AtSignIcon, PaperclipIcon, MicIcon].map(
                (Icon, i) => (
                  <button
                    key={i}
                    className="rounded p-1 text-muted-foreground/50 transition-colors hover:bg-muted/50 hover:text-muted-foreground"
                  >
                    <Icon className="h-4 w-4" />
                  </button>
                ),
              )}
            </div>
            <Button
              size="icon"
              onClick={handleSend}
              disabled={!input.trim() || sending || !isConnected}
              className="h-7 w-7 shrink-0 rounded-lg"
            >
              {sending ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <SendHorizontal className="h-3.5 w-3.5" />
              )}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
