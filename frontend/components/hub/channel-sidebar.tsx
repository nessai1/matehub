"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { Hash, Mic, Radio, LogOut, ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

type ChannelType = "text" | "voice" | "stage";

interface Channel {
  id: string;
  name: string;
  type: ChannelType;
}

const channels: Channel[] = [
  { id: "8001", name: "general", type: "text" },
  { id: "8002", name: "random", type: "text" },
  { id: "8003", name: "voice-test", type: "voice" },
  { id: "8004", name: "stage-test", type: "stage" },
];

const channelIcons: Record<ChannelType, typeof Hash> = {
  text: Hash,
  voice: Mic,
  stage: Radio,
};

export function ChannelSidebar() {
  const pathname = usePathname();

  const textChannels = channels.filter((c) => c.type === "text");
  const voiceChannels = channels.filter(
    (c) => c.type === "voice" || c.type === "stage",
  );

  return (
    <TooltipProvider delayDuration={300}>
      <aside className="flex w-56 shrink-0 flex-col border-r bg-card/50">
        {/* Hub name + exit */}
        <div className="flex h-12 items-center justify-between border-b px-3">
          <button className="flex items-center gap-1.5 rounded-md px-1.5 py-1 text-sm font-semibold transition-colors hover:bg-accent">
            Dev Hub
            <ChevronDown className="h-3.5 w-3.5 text-muted-foreground" />
          </button>
          <Tooltip>
            <TooltipTrigger asChild>
              <Link
                href="/"
                className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              >
                <LogOut className="h-3.5 w-3.5" />
              </Link>
            </TooltipTrigger>
            <TooltipContent side="bottom">Leave Hub</TooltipContent>
          </Tooltip>
        </div>

        {/* Channel list */}
        <ScrollArea className="flex-1">
          <nav className="p-2">
            <ChannelGroup label="Text Channels">
              {textChannels.map((channel) => (
                <ChannelLink
                  key={channel.id}
                  channel={channel}
                  active={pathname?.includes(channel.id) ?? false}
                />
              ))}
            </ChannelGroup>

            <Separator className="my-2" />

            <ChannelGroup label="Voice Channels">
              {voiceChannels.map((channel) => (
                <ChannelLink
                  key={channel.id}
                  channel={channel}
                  active={pathname?.includes(channel.id) ?? false}
                />
              ))}
            </ChannelGroup>
          </nav>
        </ScrollArea>
      </aside>
    </TooltipProvider>
  );
}

function ChannelGroup({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="mb-1 px-2 pt-2 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
      <div className="space-y-0.5">{children}</div>
    </div>
  );
}

// Mock: who's in each voice channel. Will come from presence API.
const voiceParticipants: Record<string, string[]> = {
  "8003": [],
  "8004": [],
};

function ChannelLink({
  channel,
  active,
}: {
  channel: Channel;
  active: boolean;
}) {
  const Icon = channelIcons[channel.type];
  const isVoice = channel.type === "voice" || channel.type === "stage";
  const participants = isVoice ? voiceParticipants[channel.id] ?? [] : [];

  return (
    <div>
      <Link
        href={`/hub/channel/${channel.id}`}
        className={cn(
          "flex items-center gap-1.5 rounded-md px-2 py-1.5 text-[13px] transition-colors",
          active
            ? "bg-primary/10 font-medium text-primary"
            : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
        )}
      >
        <Icon className="h-4 w-4 shrink-0 opacity-60" />
        {channel.name}
      </Link>
      {isVoice && participants.length > 0 && (
        <div className="ml-6 space-y-0.5 pb-1">
          {participants.map((name) => (
            <div
              key={name}
              className="flex items-center gap-1.5 px-2 py-0.5 text-[11px] text-muted-foreground"
            >
              <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
              {name}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
