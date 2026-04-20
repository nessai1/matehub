"use client";

import { ChevronUpIcon, ChevronDownIcon } from "lucide-react";
import { TextChannelView } from "@/components/hub/text-channel-view";
import { useWorkspaceStack } from "@/components/hub/workspace-stack";
import { ChannelIcon, type Channel } from "@/components/nav-channels";

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

// ── ChatWorkspace: the bottom workspace when in a voice call ───────────────

interface ChatWorkspaceProps {
  channel: Channel;
  /** If true, render without the workspace chrome (plain full-height chat). */
  standalone?: boolean;
}

export function ChatWorkspace({ channel, standalone }: ChatWorkspaceProps) {
  const { collapse } = useWorkspaceStack();

  if (standalone) {
    return (
      <div className="flex h-full min-h-0 flex-col bg-background">
        <TextChannelView
          key={channel.id}
          channelId={channel.id}
          channelName={channel.name}
        />
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <header className="flex h-11 shrink-0 items-center gap-3 border-b border-border/60 px-4">
        <div className="inline-flex items-center gap-2">
          <ChannelIcon channel={channel} />
          <span className="text-sm font-semibold text-foreground">
            {channel.name}
          </span>
        </div>
        <div className="ml-auto">
          <HeaderIconButton title="Collapse" onClick={collapse}>
            <ChevronUpIcon className="h-3.5 w-3.5" />
          </HeaderIconButton>
        </div>
      </header>
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
