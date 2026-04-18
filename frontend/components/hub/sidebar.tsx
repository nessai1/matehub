"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { Hash, Mic, Radio } from "lucide-react";
import { cn } from "@/lib/utils";

type ChannelType = "text" | "voice" | "stage";

interface Channel {
  id: string;
  name: string;
  type: ChannelType;
}

// Hard-coded for now. Will fetch from API.
const channels: Channel[] = [
  { id: "general", name: "general", type: "text" },
  { id: "random", name: "random", type: "text" },
  { id: "voice-test", name: "voice-test", type: "voice" },
  { id: "stage-test", name: "stage-test", type: "stage" },
];

const channelIcons: Record<ChannelType, typeof Hash> = {
  text: Hash,
  voice: Mic,
  stage: Radio,
};

export function Sidebar() {
  const pathname = usePathname();

  return (
    <aside className="flex w-60 flex-col border-r bg-muted/30">
      {/* Hub header */}
      <div className="flex h-12 items-center border-b px-4">
        <h1 className="text-sm font-semibold">Dev Hub</h1>
      </div>

      {/* Channel list */}
      <nav className="flex-1 overflow-y-auto p-2">
        <div className="mb-2 px-2 text-xs font-medium uppercase text-muted-foreground">
          Text Channels
        </div>
        {channels
          .filter((c) => c.type === "text")
          .map((channel) => (
            <ChannelLink
              key={channel.id}
              channel={channel}
              active={pathname?.includes(channel.id) ?? false}
            />
          ))}

        <div className="mb-2 mt-4 px-2 text-xs font-medium uppercase text-muted-foreground">
          Voice Channels
        </div>
        {channels
          .filter((c) => c.type === "voice" || c.type === "stage")
          .map((channel) => (
            <ChannelLink
              key={channel.id}
              channel={channel}
              active={pathname?.includes(channel.id) ?? false}
            />
          ))}
      </nav>

      {/* User panel */}
      <div className="flex h-14 items-center border-t bg-muted/50 px-3">
        <div className="h-8 w-8 rounded-full bg-primary/20" />
        <div className="ml-2">
          <div className="text-sm font-medium">Alice</div>
          <div className="text-xs text-muted-foreground">admin</div>
        </div>
      </div>
    </aside>
  );
}

function ChannelLink({
  channel,
  active,
}: {
  channel: Channel;
  active: boolean;
}) {
  const Icon = channelIcons[channel.type];

  return (
    <Link
      href={`/hub/channel/${channel.id}`}
      className={cn(
        "flex items-center rounded-md px-2 py-1.5 text-sm transition-colors",
        active
          ? "bg-accent text-accent-foreground"
          : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
      )}
    >
      <Icon className="mr-2 h-4 w-4 shrink-0" />
      {channel.name}
    </Link>
  );
}
