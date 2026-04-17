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
  EllipsisVerticalIcon,
} from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { MemberCard } from "@/components/hub/member-card";
import { useChatClient } from "@/hooks/use-chat-client";
import { useMembers, type Member, type MemberGroup } from "@/hooks/use-members";
import { useAuth } from "@/lib/auth";
import { cn } from "@/lib/utils";
import { mdActions, applyMarkdown, renderMarkdown } from "@/lib/markdown";
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
      <div className="group relative py-0.5 pl-11 hover:bg-muted/30">
        <span className="pointer-events-none absolute right-2 top-1 text-[10px] text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100">
          {formatTime(message.message_id)}
        </span>
        <div className="min-w-0">
          <div
            className="text-sm leading-relaxed break-words [&_strong]:font-bold [&_em]:italic [&_a]:text-primary [&_a]:underline"
            dangerouslySetInnerHTML={{ __html: renderMarkdown(message.content) }}
          />
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
  const [externalLink, setExternalLink] = useState<string | null>(null);
  const [sendOnEnter, setSendOnEnter] = useState(() => {
    if (typeof window === "undefined") return true;
    const stored = localStorage.getItem("matehub_send_on_enter");
    return stored === null ? true : stored === "1";
  });
  const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent);
  const modKey = isMac ? "Cmd" : "Ctrl";
  const scrollRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
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

  // Auto-grow textarea + container as content grows
  const autoGrow = useCallback((el: HTMLTextAreaElement) => {
    el.style.height = "auto";
    el.style.height = el.scrollHeight + "px";
    // Also grow the outer container if it has a fixed height from drag
    const container = el.closest("[data-input-container]") as HTMLElement | null;
    if (container && container.style.height) {
      const minH = parseInt(container.style.height, 10);
      const naturalH = container.scrollHeight;
      if (naturalH > minH) {
        container.style.height = Math.min(naturalH, 400) + "px";
      }
    }
  }, []);

  const applyFormat = useCallback((actionKey: string) => {
    const action = mdActions[actionKey];
    if (!action || !textareaRef.current) return;
    const newValue = applyMarkdown(textareaRef.current, action);
    setInput(newValue);
  }, []);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      // Send: Enter (default) or Ctrl/Cmd+Enter (alternative)
      if (e.key === "Enter") {
        const mod = e.ctrlKey || e.metaKey;
        const shouldSend = sendOnEnter ? !e.shiftKey && !mod : mod;
        if (shouldSend) {
          e.preventDefault();
          handleSend();
          return;
        }
      }

      // Shift+Enter: auto-continue list items
      if (e.key === "Enter" && e.shiftKey) {
        const ta = e.currentTarget;
        const { selectionStart, value } = ta;
        const lineStart = value.lastIndexOf("\n", selectionStart - 1) + 1;
        const currentLine = value.slice(lineStart, selectionStart);

        const ulMatch = currentLine.match(/^(\s*[-*]\s)/);
        const olMatch = currentLine.match(/^(\s*)(\d+)\.\s/);

        if (ulMatch || olMatch) {
          e.preventDefault();
          let prefix: string;
          if (olMatch) {
            const nextNum = parseInt(olMatch[2], 10) + 1;
            prefix = `${olMatch[1]}${nextNum}. `;
          } else {
            prefix = ulMatch![1];
          }
          const insert = "\n" + prefix;
          const newValue = value.slice(0, selectionStart) + insert + value.slice(selectionStart);
          setInput(newValue);
          requestAnimationFrame(() => {
            ta.selectionStart = ta.selectionEnd = selectionStart + insert.length;
            autoGrow(ta);
          });
        }
      }

      // Ctrl/Cmd + B/I/U shortcuts
      if (e.ctrlKey || e.metaKey) {
        const key = e.key.toLowerCase();
        if (key === "b") { e.preventDefault(); applyFormat("bold"); }
        else if (key === "i") { e.preventDefault(); applyFormat("italic"); }
        else if (key === "u") { e.preventDefault(); applyFormat("underline"); }
      }
    },
    [handleSend, applyFormat, autoGrow, sendOnEnter],
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

  // Intercept clicks on external links in messages
  const handleMessageAreaClick = useCallback((e: React.MouseEvent) => {
    const target = e.target as HTMLElement;
    const anchor = target.closest("a[data-external-link]") as HTMLAnchorElement | null;
    if (anchor) {
      e.preventDefault();
      setExternalLink(anchor.href);
    }
  }, []);

  const isConnected = connectionState === "connected";
  const isConnecting = connectionState === "connecting" || connectionState === "identifying" || connectionState === "resuming";

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
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
        className="min-h-0 flex-1 overflow-y-auto px-4"
        onScroll={handleScroll}
        onClick={handleMessageAreaClick}
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
        <div data-input-container className="flex flex-col rounded-xl border border-border/50 bg-muted/20 transition-colors focus-within:border-border">
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
            {([
              ["bold", BoldIcon],
              ["italic", ItalicIcon],
              ["underline", UnderlineIcon],
              ["link", LinkIcon],
              ["orderedList", ListOrderedIcon],
              ["unorderedList", ListIcon],
              ["code", CodeIcon],
            ] as const).map(([key, Icon]) => (
              <button
                key={key}
                onMouseDown={(e) => {
                  e.preventDefault(); // keep textarea focus
                  applyFormat(key);
                }}
                className="rounded p-1 text-muted-foreground/50 transition-colors hover:bg-muted/50 hover:text-muted-foreground"
                title={key}
              >
                <Icon className="h-3.5 w-3.5" />
              </button>
            ))}
          </div>

          {/* Middle: textarea (auto-grows with content, also grows on drag) */}
          <div className="flex-1 overflow-auto px-3 py-1">
            <textarea
              ref={textareaRef}
              value={input}
              onChange={(e) => {
                handleInput(e.target.value);
                autoGrow(e.currentTarget);
              }}
              onKeyDown={handleKeyDown}
              placeholder={`Type something... # ${channelName}`}
              disabled={!isConnected}
              rows={1}
              className={cn(
                "min-h-[2.5rem] w-full resize-none bg-transparent text-sm outline-none",
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
            <div className="flex items-center gap-0.5">
              <Popover>
                <PopoverTrigger asChild>
                  <button className="rounded p-1 text-muted-foreground/50 transition-colors hover:bg-muted/50 hover:text-muted-foreground">
                    <EllipsisVerticalIcon className="h-4 w-4" />
                  </button>
                </PopoverTrigger>
                <PopoverContent side="top" align="end" className="w-56 p-3">
                  <div className="flex items-center justify-between">
                    <div className="text-xs">
                      <p className="font-medium text-foreground">Send on Enter</p>
                      <p className="mt-0.5 text-muted-foreground">
                        {sendOnEnter
                          ? "Enter to send"
                          : `${modKey}+Enter to send`}
                      </p>
                    </div>
                    <button
                      onClick={() => {
                        const next = !sendOnEnter;
                        setSendOnEnter(next);
                        localStorage.setItem("matehub_send_on_enter", next ? "1" : "0");
                      }}
                      className={cn(
                        "relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full transition-colors",
                        sendOnEnter ? "bg-primary" : "bg-muted",
                      )}
                    >
                      <span
                        className={cn(
                          "pointer-events-none inline-block h-4 w-4 translate-y-0.5 rounded-full bg-white shadow transition-transform",
                          sendOnEnter ? "translate-x-4.5" : "translate-x-0.5",
                        )}
                      />
                    </button>
                  </div>
                </PopoverContent>
              </Popover>

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

      {/* External link confirmation */}
      <Dialog open={!!externalLink} onOpenChange={(open: boolean) => !open && setExternalLink(null)}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle className="text-center">
              External link
            </DialogTitle>
          </DialogHeader>
          <div className="py-4 text-center">
            <p className="text-sm text-muted-foreground">
              You are about to visit an external resource
            </p>
            <p className="mt-3 rounded-md bg-muted/50 px-3 py-2 text-xs font-mono break-all text-foreground">
              {externalLink}
            </p>
          </div>
          <DialogFooter className="sm:justify-center">
            <Button
              variant="outline"
              onClick={() => setExternalLink(null)}
            >
              Cancel
            </Button>
            <Button
              onClick={() => {
                if (externalLink) window.open(externalLink, "_blank", "noopener");
                setExternalLink(null);
              }}
            >
              Open link
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
