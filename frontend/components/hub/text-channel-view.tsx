import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Hash,
  SendHorizontal,
  Loader2,
  // PlusIcon,    -- non-functional, hidden in input bar
  // SmileIcon,   -- non-functional, hidden in input bar
  // AtSignIcon,  -- non-functional, hidden in input bar
  // MicIcon,     -- non-functional, hidden in input bar
  PaperclipIcon,
  BoldIcon,
  ItalicIcon,
  UnderlineIcon,
  LinkIcon,
  ListIcon,
  ListOrderedIcon,
  CodeIcon,
  EllipsisVerticalIcon,
  UploadCloudIcon,
  PanelLeftIcon,
  PanelRightIcon,
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
import { ChannelIcon, type Channel } from "@/components/nav-channels";
import { useSidebar } from "@/components/ui/sidebar";
import { useMemberSidebar } from "@/contexts/member-sidebar-context";
import { useChatClient } from "@/hooks/use-chat-client";
import { useAttachmentUpload } from "@/hooks/use-attachment-upload";
import { useMembers, type Member, type MemberGroup } from "@/hooks/use-members";
import { MessageAttachments } from "@/components/hub/media/message-attachments";
import { UploadPreview } from "@/components/hub/media/upload-preview";
import { useAuth } from "@/lib/auth";
import { cn } from "@/lib/utils";
import { mdActions, applyMarkdown, renderMarkdown } from "@/lib/markdown";
import type { ChatMessage as ChatMessageT } from "@/contexts/chat-context";
import { t } from "@/i18n";

interface TextChannelViewProps {
  channelId: string;
  channelName: string;
  /** Optional channel for richer header rendering (custom icon image / color). */
  channel?: Channel;
  /** Skip the built-in h-10 header (used when a parent workspace provides its own). */
  hideHeader?: boolean;
}

// ── Sonyflake timestamp extraction ───────────────
// Layout: time(39 bits, 10ms units) | sequence(8) | machine(16)
// Epoch: 2014-09-01T00:00:00Z
//
// The id value exceeds JS MAX_SAFE_INTEGER — parse through BigInt, then
// convert the small (time-only) result back to Number for the Date math.

const SONYFLAKE_EPOCH_MS = 1409529600000;

function snowflakeToDate(id: string): Date {
  const time10ms = Number(BigInt(id) >> 24n);
  return new Date(SONYFLAKE_EPOCH_MS + time10ms * 10);
}

/** Truncate a Date to local midnight. Used to compare "same calendar day". */
function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** "Today" / "Yesterday" / "DD.MM.YYYY" for a message-separator row. */
function formatDateSeparator(d: Date): string {
  const today = startOfDay(new Date());
  const yesterday = today - 86400 * 1000;
  const day = startOfDay(d);
  if (day === today) return t("Today");
  if (day === yesterday) return t("Yesterday");
  const dd = String(d.getDate()).padStart(2, "0");
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  return `${dd}.${mm}.${d.getFullYear()}`;
}

function DateSeparator({ label }: { label: string }) {
  return (
    <div
      role="separator"
      className="my-3 flex items-center gap-3 px-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground"
    >
      <div className="h-px flex-1 bg-border/50" />
      <span>{label}</span>
      <div className="h-px flex-1 bg-border/50" />
    </div>
  );
}

/** Red tick that anchors where unread messages start. */
function UnreadDivider() {
  return (
    <div
      role="separator"
      className="my-2 flex items-center gap-2 px-1 text-[10px] font-semibold uppercase tracking-wider text-destructive"
    >
      <div className="h-px flex-1 bg-destructive/40" />
      <span>{t("New")}</span>
      <div className="h-px flex-1 bg-destructive/40" />
    </div>
  );
}

function formatTime(messageId: string): string {
  return snowflakeToDate(messageId).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

// ── System message row ───────────────────────────
//
// Used for chat-service system events like DM call lifecycle. Backend writes
// the content as a template (e.g. "$1 didn't answer the call") with the
// referenced user_ids in `mentions`; we splice display names in here so we
// don't need a hub-side lookup at write time.

function renderSystemContent(
  content: string,
  mentions: string[],
  memberMap: Map<string, Member>,
): React.ReactNode {
  const parts: React.ReactNode[] = [];
  const re = /\$(\d+)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(content)) !== null) {
    if (match.index > lastIndex) {
      parts.push(content.slice(lastIndex, match.index));
    }
    const idx = parseInt(match[1], 10) - 1;
    const userId = mentions[idx];
    const member = userId ? memberMap.get(userId) : undefined;
    parts.push(
      <span key={`m-${match.index}`} className="font-medium text-foreground">
        {member?.display_name ?? member?.username ?? userId ?? t("Unknown")}
      </span>,
    );
    lastIndex = match.index + match[0].length;
  }
  if (lastIndex < content.length) parts.push(content.slice(lastIndex));
  return parts;
}

function SystemMessageRow({
  message,
  memberMap,
}: {
  message: ChatMessageT;
  memberMap: Map<string, Member>;
}) {
  return (
    <div
      role="status"
      className="my-2 flex items-center gap-3 px-1 text-[11px] italic text-muted-foreground"
    >
      <div className="h-px flex-1 bg-border/40" />
      <span className="text-center">
        {renderSystemContent(message.content, message.mentions ?? [], memberMap)}
      </span>
      <div className="h-px flex-1 bg-border/40" />
    </div>
  );
}

// ── Message bubble ───────────────────────────────

function ChatMessage({
  message,
  member,
  isGrouped,
  allGroups,
  onGroupsChanged,
  onRetry,
}: {
  message: ChatMessageT;
  member?: Member;
  isGrouped: boolean;
  allGroups: MemberGroup[];
  onGroupsChanged: () => void;
  onRetry?: (clientId: string) => void;
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

  const pending = message._status === "sending";
  const failed = message._status === "failed";
  // Sending: fade + show "sending" tag. Failed: red border + retry button.
  const statusClass = pending
    ? "opacity-50"
    : failed
      ? "rounded border-l-2 border-destructive pl-2"
      : "";

  const statusBadge = pending ? (
    <span className="text-[10px] italic text-muted-foreground">{t("sending…")}</span>
  ) : failed ? (
    <span className="flex items-center gap-1 text-[10px] text-destructive">
      {t("failed")}
      {message.client_id && onRetry && (
        <button
          type="button"
          onClick={() => onRetry(message.client_id!)}
          className="underline hover:no-underline"
        >
          {t("retry")}
        </button>
      )}
    </span>
  ) : null;

  if (isGrouped) {
    return (
      <div className={cn("group relative py-0.5 pl-11 hover:bg-muted/30", statusClass)}>
        <span className="pointer-events-none absolute right-2 top-1 text-[10px] text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100">
          {formatTime(message.message_id)}
        </span>
        <div className="min-w-0">
          {message.content && (
            <div
              className="text-sm leading-relaxed break-words [&_strong]:font-bold [&_em]:italic [&_a]:text-primary [&_a]:underline"
              dangerouslySetInnerHTML={{ __html: renderMarkdown(message.content) }}
            />
          )}
          <MessageAttachments attachments={message.attachments} />
          {statusBadge && <div className="mt-0.5">{statusBadge}</div>}
        </div>
      </div>
    );
  }

  return (
    <div className={cn("group relative mt-3 flex gap-3 py-1 hover:bg-muted/30 first:mt-0", statusClass)}>
      {wrapWithCard(avatarEl)}
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          {wrapWithCard(nameEl)}
          <span className="text-[10px] text-muted-foreground">
            {formatTime(message.message_id)}
          </span>
          {message.edited_at && (
            <span className="text-[10px] text-muted-foreground">{t("(edited)")}</span>
          )}
          {statusBadge}
        </div>
        {message.content && (
          <div
            className="text-sm leading-relaxed break-words [&_strong]:font-bold [&_em]:italic [&_a]:text-primary [&_a]:underline"
            dangerouslySetInnerHTML={{ __html: renderMarkdown(message.content) }}
          />
        )}
        <MessageAttachments attachments={message.attachments} />
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
    // Both `typingUsers` and `Member.user_id` are stringified Snowflakes.
    const m = members.find((mm) => mm.user_id === uid);
    return m?.display_name || m?.username || uid;
  });

  let text: string;
  if (names.length === 1) {
    text = t("%s is typing", names[0]);
  } else if (names.length === 2) {
    text = t("%s and %s are typing", names[0], names[1]);
  } else {
    text = t("%d people are typing", names.length);
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

export function TextChannelView({ channelId, channelName, channel, hideHeader }: TextChannelViewProps) {
  const { session } = useAuth();
  const { toggleSidebar: toggleLeftSidebar, open: leftOpen, openMobile: leftOpenMobile, isMobile: leftIsMobile } =
    useSidebar();
  const { open: rightOpen, toggle: toggleRightSidebar } = useMemberSidebar();
  const leftActive = leftIsMobile ? leftOpenMobile : leftOpen;
  const {
    client,
    messages,
    connectionState,
    typingUsers,
    dividerPos,
    sendMessage,
    sendTyping,
    loadMore,
    retryMessage,
  } = useChatClient(channelId);
  const { uploads, addFiles, clearAll, readyAttachments, hasInFlight } =
    useAttachmentUpload(client, channelId);
  const { members, loading: membersLoading, refetch } = useMembers();
  const fileInputRef = useRef<HTMLInputElement>(null);
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

  // Member lookup map keyed by user_id (stringified Snowflake).
  const memberMap = useMemo(() => {
    const map = new Map<string, Member>();
    for (const m of members) map.set(m.user_id, m);
    return map;
  }, [members]);

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

  // ── Drag & drop into the input container ──
  // Drag counter trick: dragenter fires for every nested child, so a plain
  // boolean would flicker. Keep a counter, only hide when it hits zero.
  const [isDragOver, setIsDragOver] = useState(false);
  const dragCounter = useRef(0);

  const dragHasFiles = (e: React.DragEvent): boolean => {
    const types = e.dataTransfer?.types;
    if (!types) return false;
    return Array.from(types).includes("Files");
  };

  const handleDragEnter = useCallback((e: React.DragEvent) => {
    if (!dragHasFiles(e)) return;
    e.preventDefault();
    dragCounter.current += 1;
    setIsDragOver(true);
  }, []);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    if (!dragHasFiles(e)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    if (!dragHasFiles(e)) return;
    e.preventDefault();
    dragCounter.current = Math.max(0, dragCounter.current - 1);
    if (dragCounter.current === 0) setIsDragOver(false);
  }, []);

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      if (!dragHasFiles(e)) return;
      e.preventDefault();
      dragCounter.current = 0;
      setIsDragOver(false);
      if (e.dataTransfer.files.length) addFiles(e.dataTransfer.files);
    },
    [addFiles],
  );

  // ── Clipboard paste (Ctrl+V) ──
  // Image-from-screenshot lands as a single file with empty `name` field.
  // Synthesize one so the server has a sensible filename to round-trip.
  const handlePaste = useCallback(
    (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
      const items = e.clipboardData?.items;
      if (!items || items.length === 0) return;
      const files: File[] = [];
      for (const item of Array.from(items)) {
        if (item.kind !== "file") continue;
        const f = item.getAsFile();
        if (!f) continue;
        if (!f.name || f.name === "image.png") {
          // Most browsers name pasted images "image.png" — give each a unique
          // timestamp so they don't collide in the previews list.
          const ext = (f.type.split("/")[1] || "bin").replace("+xml", "");
          files.push(new File([f], `pasted-${Date.now()}.${ext}`, { type: f.type }));
        } else {
          files.push(f);
        }
      }
      if (files.length > 0) {
        e.preventDefault(); // don't also paste binary garbage as text
        addFiles(files);
      }
    },
    [addFiles],
  );

  const handleSend = useCallback(async () => {
    const text = input;
    const hasText = text.trim().length > 0;
    const hasAttachments = readyAttachments.length > 0;
    if ((!hasText && !hasAttachments) || sending || hasInFlight) return;

    setInput("");
    setSending(true);
    try {
      await sendMessage(text, hasAttachments ? readyAttachments : undefined);
      clearAll();
    } catch (err) {
      console.error("send failed:", err);
      if (hasText) setInput(text); // restore text on failure
    } finally {
      setSending(false);
    }
  }, [input, sending, sendMessage, readyAttachments, hasInFlight, clearAll]);

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

  function isGroupedWith(prev: ChatMessageT | undefined, curr: ChatMessageT): boolean {
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
    <div
      className="relative flex min-h-0 flex-1 flex-col overflow-hidden"
      onDragEnter={handleDragEnter}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
    >
      {!hideHeader && (
        <header className="flex h-10 shrink-0 items-center gap-2 border-b px-2">
          <button
            type="button"
            title={t("Toggle channels")}
            onClick={toggleLeftSidebar}
            className={cn(
              "grid h-7 w-7 place-items-center rounded-md transition-colors hover:bg-muted hover:text-foreground",
              leftActive ? "text-foreground" : "text-muted-foreground",
            )}
          >
            <PanelLeftIcon className="h-3.5 w-3.5" />
          </button>
          {channel ? (
            <ChannelIcon channel={channel} />
          ) : (
            <Hash className="h-4 w-4 text-muted-foreground" />
          )}
          <span className="text-sm font-medium">{channelName}</span>
          <div className="ml-auto flex items-center gap-2">
            {!isConnected && (
              <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
                {isConnecting ? (
                  <>
                    <Loader2 className="h-3 w-3 animate-spin" />
                    {t("Connecting...")}
                  </>
                ) : connectionState === "reconnecting" ? (
                  <>
                    <Loader2 className="h-3 w-3 animate-spin" />
                    {t("Reconnecting...")}
                  </>
                ) : (
                  <span className="text-destructive">{t("Disconnected")}</span>
                )}
              </span>
            )}
            <button
              type="button"
              title={t("Toggle members")}
              onClick={toggleRightSidebar}
              className={cn(
                "grid h-7 w-7 place-items-center rounded-md transition-colors hover:bg-muted hover:text-foreground",
                rightOpen ? "text-foreground" : "text-muted-foreground",
              )}
            >
              <PanelRightIcon className="h-3.5 w-3.5" />
            </button>
          </div>
        </header>
      )}

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
                {t("Welcome to #%s", channelName)}
              </p>
              <p className="mt-1 text-xs">
                {t("This is the start of the channel.")}
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
            {messages.map((msg, i) => {
              const prev = messages[i - 1];
              const msgDate = snowflakeToDate(msg.message_id);
              const newDay =
                !prev ||
                startOfDay(msgDate) !== startOfDay(snowflakeToDate(prev.message_id));
              // "New messages" divider: first message whose id exceeds the
              // snapshot taken on channel entry. BigInt compare because
              // Snowflakes don't fit into a JS number.
              const crossedUnread =
                dividerPos != null &&
                BigInt(msg.message_id) > BigInt(dividerPos) &&
                (!prev || BigInt(prev.message_id) <= BigInt(dividerPos));
              // On a day boundary we reset grouping so the first message of the
              // day always shows avatar + name, never the compact variant.
              // Same on an unread boundary, and on a system-message boundary
              // (otherwise an Alice-Alice pair with a "Call ended" line in
              // between would still group, which reads weird).
              const prevIsSystem = prev?.author_id === "system";
              const grouped =
                !newDay && !crossedUnread && !prevIsSystem && isGroupedWith(prev, msg);
              const isSystem = msg.author_id === "system";
              return (
                <Fragment key={msg.client_id ?? msg.message_id}>
                  {newDay && <DateSeparator label={formatDateSeparator(msgDate)} />}
                  {crossedUnread && !newDay && <UnreadDivider />}
                  {isSystem ? (
                    <SystemMessageRow message={msg} memberMap={memberMap} />
                  ) : (
                    <ChatMessage
                      message={msg}
                      member={memberMap.get(msg.author_id)}
                      isGrouped={grouped}
                      allGroups={allGroups}
                      onGroupsChanged={refetch}
                      onRetry={retryMessage}
                    />
                  )}
                </Fragment>
              );
            })}
          </div>
        )}
        <div ref={bottomRef} />
      </div>

      {/* ── Typing indicator ── */}
      <TypingIndicator users={typingUsers} members={members} />

      {/* ── Input ── */}
      <div className="px-4 pb-4">
        <div
          data-input-container
          className="flex flex-col rounded-xl border border-border/50 bg-muted/20 transition-colors focus-within:border-border"
        >
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

          {/* Upload previews (above textarea) */}
          <UploadPreview uploads={uploads} />

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
              onPaste={handlePaste}
              placeholder={t("Type something... # %s", channelName)}
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
              <button
                className="rounded p-1 text-muted-foreground/50 transition-colors hover:bg-muted/50 hover:text-muted-foreground"
                title={t("Attach file")}
                onClick={() => fileInputRef.current?.click()}
              >
                <PaperclipIcon className="h-4 w-4" />
              </button>
              <input
                ref={fileInputRef}
                type="file"
                multiple
                className="hidden"
                onChange={(e) => {
                  if (e.target.files) addFiles(e.target.files);
                  e.target.value = ""; // allow re-selecting same file
                }}
              />
              {/* Plus / Smile / AtSign / Mic hidden until they have real
                  handlers. File upload (paperclip above) is the only wired
                  non-text input action right now.
              {[PlusIcon, SmileIcon, AtSignIcon, MicIcon].map((Icon, i) => (
                <button
                  key={i}
                  className="rounded p-1 text-muted-foreground/50 transition-colors hover:bg-muted/50 hover:text-muted-foreground"
                >
                  <Icon className="h-4 w-4" />
                </button>
              ))} */}
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
                      <p className="font-medium text-foreground">{t("Send on Enter")}</p>
                      <p className="mt-0.5 text-muted-foreground">
                        {sendOnEnter
                          ? t("Enter to send")
                          : t("%s+Enter to send", modKey)}
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
                disabled={(!input.trim() && readyAttachments.length === 0) || sending || !isConnected || hasInFlight}
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

      {/* Workspace-wide drag overlay: covers the whole channel area. Pointer
          events disabled so dragenter/over/leave/drop keep flowing to the
          parent — otherwise the overlay would intercept the leave/drop and
          the counter would never settle. */}
      {isDragOver && (
        <div className="pointer-events-none absolute inset-0 z-30 flex flex-col items-center justify-center gap-3 rounded-md border-2 border-dashed border-primary/70 bg-background/80 backdrop-blur-sm">
          <UploadCloudIcon className="h-12 w-12 text-primary" />
          <span className="text-base font-medium text-primary">
            {t("Drop to attach")}
          </span>
          <span className="text-xs text-muted-foreground">
            {t("Files upload to #%s", channelName)}
          </span>
        </div>
      )}

      {/* External link confirmation */}
      <Dialog open={!!externalLink} onOpenChange={(open: boolean) => !open && setExternalLink(null)}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle className="text-center">
              {t("External link")}
            </DialogTitle>
          </DialogHeader>
          <div className="py-4 text-center">
            <p className="text-sm text-muted-foreground">
              {t("You are about to visit an external resource")}
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
              {t("Cancel")}
            </Button>
            <Button
              onClick={() => {
                if (externalLink) window.open(externalLink, "_blank", "noopener");
                setExternalLink(null);
              }}
            >
              {t("Open link")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
