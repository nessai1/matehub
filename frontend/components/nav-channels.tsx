"use client"

import Link from "next/link"
import { usePathname } from "next/navigation"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar"
import {
  HashIcon,
  MicIcon,
  RadioIcon,
  PlusIcon,
  ChevronRightIcon,
  UserPlusIcon,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { useAuth } from "@/lib/auth"
import { useMembers } from "@/hooks/use-members"

type ChannelType = "text" | "voice" | "stage"

interface Channel {
  id: string
  name: string
  type: ChannelType
}

const channelIcons: Record<ChannelType, React.ElementType> = {
  text: HashIcon,
  voice: MicIcon,
  stage: RadioIcon,
}

// TODO: fetch from hub API
const channels: Channel[] = [
  { id: "8001", name: "general", type: "text" },
  { id: "8002", name: "random", type: "text" },
  { id: "8003", name: "voice-test", type: "voice" },
  { id: "8004", name: "stage-test", type: "stage" },
]

// ── Channel link ─────────────────────────────────

function ChannelLink({ channel, active }: { channel: Channel; active: boolean }) {
  const Icon = channelIcons[channel.type]

  return (
    <li>
      <Link
        href={`/hub/channel/${channel.id}`}
        className={cn(
          "flex items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-all",
          active
            ? "border-l-[3px] border-sidebar-primary bg-sidebar-accent font-semibold text-sidebar-foreground"
            : "border-l-[3px] border-transparent text-sidebar-foreground/60 hover:bg-sidebar-accent/50 hover:text-sidebar-foreground",
        )}
      >
        <Icon className="h-4 w-4 shrink-0" />
        <span className="truncate">{channel.name}</span>
      </Link>
    </li>
  )
}

// ── Add button ───────────────────────────────────

function AddButton({ label }: { label: string }) {
  return (
    <li>
      <button className="flex w-full items-center gap-1.5 px-2 py-1 text-xs text-sidebar-foreground/40 transition-colors hover:text-sidebar-foreground/70">
        <PlusIcon className="h-3 w-3" />
        <span>{label}</span>
      </button>
    </li>
  )
}

// ── Collapsible section ──────────────────────────

function ChannelSection({
  label,
  children,
  defaultOpen = true,
}: {
  label: string
  children: React.ReactNode
  defaultOpen?: boolean
}) {
  return (
    <Collapsible defaultOpen={defaultOpen}>
      <div className="px-2">
        <CollapsibleTrigger className="flex w-full items-center gap-1 py-1.5 text-[11px] font-semibold uppercase tracking-wider text-sidebar-foreground/50 hover:text-sidebar-foreground/80">
          <ChevronRightIcon className="h-3 w-3 transition-transform duration-200 [[data-state=open]_&]:rotate-90" />
          {label}
        </CollapsibleTrigger>
        <CollapsibleContent>
          <ul className="flex flex-col gap-0.5">{children}</ul>
        </CollapsibleContent>
      </div>
    </Collapsible>
  )
}

// ── Main export ──────────────────────────────────

export function NavChannels() {
  const pathname = usePathname()
  const { session } = useAuth()
  const { members } = useMembers()

  const textChannels = channels.filter((c) => c.type === "text")
  const voiceChannels = channels.filter(
    (c) => c.type === "voice" || c.type === "stage",
  )

  // Online members for DM section (exclude self)
  const dmMembers = members.filter((m) => m.user_id !== session?.userId)

  return (
    <>
      {/* ── Text Channels ── */}
      <ChannelSection label="Channels">
        {textChannels.map((ch) => (
          <ChannelLink
            key={ch.id}
            channel={ch}
            active={pathname === `/hub/channel/${ch.id}`}
          />
        ))}
        <AddButton label="Add Channel" />
      </ChannelSection>

      {/* ── Voice Channels ── */}
      <ChannelSection label="Voice Channels">
        {voiceChannels.map((ch) => (
          <ChannelLink
            key={ch.id}
            channel={ch}
            active={pathname === `/hub/channel/${ch.id}`}
          />
        ))}
        <AddButton label="Add Channel" />
      </ChannelSection>

      {/* ── Direct Messages ── */}
      <ChannelSection label="Direct Messages">
        {dmMembers.map((member) => (
          <li key={member.user_id}>
            <button className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-sm text-sidebar-foreground/60 transition-colors hover:bg-sidebar-accent/50 hover:text-sidebar-foreground">
              <div className="relative">
                <Avatar size="sm">
                  {member.avatar_url && <AvatarImage src={member.avatar_url} />}
                  <AvatarFallback className="text-[10px]">
                    {member.display_name.charAt(0).toUpperCase()}
                  </AvatarFallback>
                </Avatar>
                <span
                  className={cn(
                    "absolute -bottom-0.5 -right-0.5 h-2 w-2 rounded-full border border-sidebar",
                    member.is_online ? "bg-emerald-500" : "bg-zinc-500",
                  )}
                />
              </div>
              <span className="truncate">{member.display_name}</span>
            </button>
          </li>
        ))}
        {/* Current user */}
        {session && (
          <li>
            <div className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm text-sidebar-foreground/40">
              <div className="relative">
                <Avatar size="sm">
                  {session.avatarUrl && <AvatarImage src={session.avatarUrl} />}
                  <AvatarFallback className="text-[10px]">
                    {session.displayName.charAt(0).toUpperCase()}
                  </AvatarFallback>
                </Avatar>
                <span className="absolute -bottom-0.5 -right-0.5 h-2 w-2 rounded-full border border-sidebar bg-emerald-500" />
              </div>
              <span className="truncate">{session.displayName}</span>
              <span className="ml-auto text-[10px] text-sidebar-foreground/30">(You)</span>
            </div>
          </li>
        )}
        <li>
          <button className="flex w-full items-center gap-1.5 px-2 py-1 text-xs text-sidebar-foreground/40 transition-colors hover:text-sidebar-foreground/70">
            <UserPlusIcon className="h-3 w-3" />
            <span>Add Teammates</span>
          </button>
        </li>
      </ChannelSection>
    </>
  )
}
