import { ChevronUpIcon, ChevronDownIcon, PhoneIcon } from "lucide-react";
import { TextChannelView } from "@/components/hub/text-channel-view";
import { useWorkspaceStack } from "@/components/hub/workspace-stack";
import { ChannelIcon, type Channel } from "@/components/nav-channels";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { useAuth } from "@/lib/auth";
import { useMembers } from "@/hooks/use-members";
import { useIncomingCall } from "@/contexts/incoming-call-context";
import { useVideoCall } from "@/contexts/video-call-context";
import { cn } from "@/lib/utils";

// ── Icon-only square button in the header ──────────────────────────────────

function HeaderIconButton({
  children,
  onClick,
  title,
}: {
  children: React.ReactNode;
  onClick?: () => void;
  title?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className="grid h-7 w-7 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
    >
      {children}
    </button>
  );
}

// ── DM-aware header pieces ────────────────────────────────────────────────
//
// For type='dm' channels the conventional "# name" header doesn't help — the
// channel has no name, and the only useful identity is the peer. Render
// avatar + display name + Call button instead. Reused by both the standalone
// path and the docked workspace.

function useDmPeer(channel: Channel) {
  const { session } = useAuth();
  const { members } = useMembers();
  if (channel.type !== "dm") return null;
  const peerId = channel.participants?.find((id) => id !== session?.userId);
  if (!peerId) return null;
  return {
    peerId,
    member: members.find((m) => m.user_id === peerId),
  };
}

function DmHeaderInner({
  channel,
  rightExtras,
}: {
  channel: Channel;
  rightExtras?: React.ReactNode;
}) {
  const peer = useDmPeer(channel);
  const { startCall } = useIncomingCall();
  const { activeVoiceChannelId } = useVideoCall();
  const displayName = peer?.member?.display_name ?? peer?.peerId ?? "Unknown";
  // Once we're already in this DM's SFU session the workspace splits in two
  // (VideoWorkspace on top, ChatWorkspace on bottom). The Call button on the
  // bottom header would re-trigger ringing — replace it with a passive badge.
  const inThisCall = activeVoiceChannelId === channel.id;
  return (
    <header className="flex h-11 shrink-0 items-center gap-3 border-b border-border/60 px-4">
      <div className="inline-flex items-center gap-2">
        <div className="relative">
          <Avatar size="sm">
            {peer?.member?.avatar_url && <AvatarImage src={peer.member.avatar_url} />}
            <AvatarFallback
              className={cn(
                "text-[10px] font-medium",
                peer?.member?.is_online
                  ? "bg-primary/15 text-primary"
                  : "bg-muted text-muted-foreground",
              )}
            >
              {displayName.charAt(0).toUpperCase()}
            </AvatarFallback>
          </Avatar>
          <span
            className={cn(
              "absolute -bottom-0.5 -right-0.5 h-2 w-2 rounded-full border border-background",
              peer?.member?.is_online ? "bg-emerald-500" : "bg-zinc-500",
            )}
          />
        </div>
        <span className="text-sm font-semibold text-foreground">
          {displayName}
        </span>
      </div>
      <div className="ml-auto inline-flex items-center gap-1">
        {inThisCall ? (
          <span className="inline-flex items-center gap-1.5 rounded-full bg-emerald-500/15 px-2.5 py-1 text-[11px] font-semibold text-emerald-500">
            <PhoneIcon className="h-3 w-3" />
            in call
          </span>
        ) : (
          <HeaderIconButton title="Call" onClick={() => void startCall(channel.id)}>
            <PhoneIcon className="h-3.5 w-3.5" />
          </HeaderIconButton>
        )}
        {rightExtras}
      </div>
    </header>
  );
}

function ChannelHeaderInner({
  channel,
  rightExtras,
}: {
  channel: Channel;
  rightExtras?: React.ReactNode;
}) {
  return (
    <header className="flex h-11 shrink-0 items-center gap-3 border-b border-border/60 px-4">
      <div className="inline-flex items-center gap-2">
        <ChannelIcon channel={channel} />
        <span className="text-sm font-semibold text-foreground">
          {channel.name}
        </span>
      </div>
      {rightExtras && <div className="ml-auto">{rightExtras}</div>}
    </header>
  );
}

// ── ChatWorkspace: the bottom workspace when in a voice call ───────────────

interface ChatWorkspaceProps {
  channel: Channel;
  /** If true, render without the workspace chrome (plain full-height chat). */
  standalone?: boolean;
}

export function ChatWorkspace({ channel, standalone }: ChatWorkspaceProps) {
  const { collapse } = useWorkspaceStack();
  const isDm = channel.type === "dm";

  // Standalone path: still show the DM header (call button is the whole point);
  // for regular channels we keep the legacy behavior of letting TextChannelView
  // render its own header.
  if (standalone) {
    return (
      <div className="flex h-full min-h-0 flex-col bg-background">
        {isDm && <DmHeaderInner channel={channel} />}
        <TextChannelView
          key={channel.id}
          channelId={channel.id}
          channelName={channel.name}
          channel={channel}
          hideHeader={isDm}
        />
      </div>
    );
  }

  const collapseBtn = (
    <HeaderIconButton title="Collapse" onClick={collapse}>
      <ChevronUpIcon className="h-3.5 w-3.5" />
    </HeaderIconButton>
  );

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      {isDm ? (
        <DmHeaderInner channel={channel} rightExtras={collapseBtn} />
      ) : (
        <ChannelHeaderInner channel={channel} rightExtras={collapseBtn} />
      )}
      <div className="flex min-h-0 flex-1 flex-col">
        <TextChannelView
          key={channel.id}
          channelId={channel.id}
          channelName={channel.name}
          hideHeader
        />
      </div>
    </div>
  );
}

// ── Collapsed chat bar (shown instead of the bottom workspace) ─────────────

export function CollapsedChatBar({
  channel,
  unread,
  mentions,
}: {
  channel: Channel;
  unread?: number;
  mentions?: number;
}) {
  const { expand } = useWorkspaceStack();
  return (
    <button
      type="button"
      onClick={expand}
      className="flex h-8 w-full shrink-0 items-center gap-3 border-t border-border/60 bg-background px-4 text-left transition-colors hover:bg-muted/40"
    >
      <ChannelIcon channel={channel} />
      <span className="text-[13px] font-semibold text-foreground">
        {channel.name}
      </span>
      <span className="text-[11px] text-muted-foreground">· Chat</span>
      {unread != null && unread > 0 && (
        <span className="inline-flex items-center rounded-full bg-[oklch(0.62_0.22_20)] px-1.5 py-[1px] font-mono text-[10px] font-bold text-white">
          {unread} new
        </span>
      )}
      {mentions != null && mentions > 0 && (
        <span className="inline-flex items-center rounded-full bg-[oklch(0.7_0.18_50)] px-1.5 py-[1px] font-mono text-[10px] font-bold text-[#1a1000]">
          @{mentions}
        </span>
      )}
      <span className="ml-auto inline-flex items-center gap-1 rounded-md border border-border/60 px-2.5 py-1 text-[11px] text-muted-foreground">
        <ChevronDownIcon className="h-3 w-3" />
        expand
      </span>
    </button>
  );
}
