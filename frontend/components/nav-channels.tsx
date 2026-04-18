"use client"

import { useCallback, useState } from "react"
import { useChannels, type Channel as ApiChannel } from "@/hooks/use-channels"
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
  SettingsIcon,
  MessageSquareIcon,
  BookOpenIcon,
  MegaphoneIcon,
  ZapIcon,
  StarIcon,
  HeartIcon,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { useAuth } from "@/lib/auth"
import { useMembers, type MemberGroup } from "@/hooks/use-members"
import { usePermissions, P } from "@/hooks/use-permissions"
import { ChannelEditor, type ChannelEditorData } from "@/components/hub/channel-editor"

type ChannelType = "text" | "voice" | "stage"

interface Channel {
  id: string
  name: string
  type: ChannelType
  position: number
  iconColor?: string | null
  iconImage?: string | null
  iconId?: string
}

const defaultIcons: Record<ChannelType, React.ElementType> = {
  text: HashIcon,
  voice: MicIcon,
  stage: RadioIcon,
}

const iconMap: Record<string, React.ElementType> = {
  hash: HashIcon,
  mic: MicIcon,
  radio: RadioIcon,
  chat: MessageSquareIcon,
  book: BookOpenIcon,
  megaphone: MegaphoneIcon,
  zap: ZapIcon,
  star: StarIcon,
  heart: HeartIcon,
}

// ── Channel icon (colored square / image / default) ──

function ChannelIcon({ channel, className }: { channel: Channel; className?: string }) {
  const Icon = (channel.iconId && iconMap[channel.iconId]) || defaultIcons[channel.type]

  if (channel.iconImage) {
    return (
      <div className={cn("h-5 w-5 shrink-0 overflow-hidden rounded", className)}>
        <img src={channel.iconImage} alt="" className="h-full w-full object-cover" />
      </div>
    )
  }
  if (channel.iconColor) {
    return (
      <div
        className={cn("flex h-5 w-5 shrink-0 items-center justify-center rounded", className)}
        style={{ backgroundColor: channel.iconColor }}
      >
        <Icon className="h-3 w-3 text-white" />
      </div>
    )
  }
  return (
    <div className={cn("flex h-5 w-5 shrink-0 items-center justify-center", className)}>
      <Icon className="h-4 w-4" />
    </div>
  )
}

// ── Channel link with gear on hover ──────────────

function ChannelLink({
  channel,
  active,
  onEdit,
}: {
  channel: Channel
  active: boolean
  onEdit?: () => void
}) {
  return (
    <li>
      <div
        className={cn(
          "group/ch flex items-center rounded-md transition-all",
          active
            ? "border-l-[3px] border-sidebar-primary bg-sidebar-accent font-semibold text-sidebar-foreground"
            : "border-l-[3px] border-transparent text-sidebar-foreground/60 hover:bg-sidebar-accent/50 hover:text-sidebar-foreground",
        )}
      >
        <Link
          href={`/hub/channel/${channel.id}`}
          className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-sm"
        >
          <ChannelIcon channel={channel} />
          <span className="truncate">{channel.name}</span>
        </Link>
        {onEdit && (
          <button
            onClick={(e) => {
              e.preventDefault()
              onEdit()
            }}
            className="mr-1 rounded p-1 text-sidebar-foreground/30 opacity-0 transition-all hover:bg-sidebar-accent hover:text-sidebar-foreground group-hover/ch:opacity-100"
          >
            <SettingsIcon className="h-3 w-3" />
          </button>
        )}
      </div>
    </li>
  )
}

// ── Add button ───────────────────────────────────

function AddButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <li>
      <button
        onClick={onClick}
        className="flex w-full items-center gap-1.5 px-2 py-1 text-xs text-sidebar-foreground/40 transition-colors hover:text-sidebar-foreground/70"
      >
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
  const { has: hasPerm } = usePermissions()

  const { channels: apiChannels, createChannel, updateChannel, deleteChannel, uploadIcon } = useChannels()

  // Map API channels to local Channel type
  const channels: Channel[] = apiChannels.map((ch) => ({
    id: ch.id,
    name: ch.name,
    type: ch.type,
    position: ch.position,
    iconId: ch.icon_id ?? undefined,
    iconColor: ch.icon_color,
    iconImage: ch.icon_image_url,
  }))

  const [editorOpen, setEditorOpen] = useState(false)
  const [editorMode, setEditorMode] = useState<"create" | "edit">("create")
  const [editorType, setEditorType] = useState<ChannelType>("text")
  const [editingChannel, setEditingChannel] = useState<Channel | null>(null)

  const textChannels = channels.filter((c) => c.type === "text")
  const voiceChannels = channels.filter((c) => c.type === "voice" || c.type === "stage")
  const dmMembers = members.filter((m) => m.user_id !== session?.userId)

  // Collect groups for editor
  const allGroups: MemberGroup[] = []
  const seen = new Set<string>()
  for (const m of members) {
    for (const g of m.groups) {
      if (!seen.has(g.id)) {
        seen.add(g.id)
        allGroups.push(g)
      }
    }
  }

  const openCreate = (type: ChannelType) => {
    setEditorMode("create")
    setEditorType(type)
    setEditingChannel(null)
    setEditorOpen(true)
  }

  const openEdit = (channel: Channel) => {
    setEditorMode("edit")
    setEditorType(channel.type)
    setEditingChannel(channel)
    setEditorOpen(true)
  }

  const handleSave = useCallback(async (data: ChannelEditorData) => {
    let targetId: string | null = null

    if (editorMode === "create") {
      const ch = await createChannel({
        name: data.name,
        type: data.type,
        icon_id: data.iconId,
        icon_color: data.iconColor,
      })
      targetId = ch?.id ?? null
    } else if (editingChannel) {
      await updateChannel(editingChannel.id, {
        name: data.name,
        icon_id: data.iconId,
        icon_color: data.iconColor,
        icon_image_url: data.iconImage,
      })
      targetId = editingChannel.id
    }

    // Upload icon file to S3 if present
    if (data.pendingIconFile && targetId) {
      await uploadIcon(targetId, data.pendingIconFile)
    }
  }, [editorMode, editingChannel, createChannel, updateChannel, uploadIcon])

  return (
    <>
      {/* ── Text Channels ── */}
      <ChannelSection label="Channels">
        {textChannels.map((ch) => (
          <ChannelLink
            key={ch.id}
            channel={ch}
            active={pathname === `/hub/channel/${ch.id}`}
            onEdit={hasPerm(P.EDIT_OTHER_CHANNELS) ? () => openEdit(ch) : undefined}
          />
        ))}
        {hasPerm(P.CREATE_TEXT_CHANNELS) && (
          <AddButton label="Add Channel" onClick={() => openCreate("text")} />
        )}
      </ChannelSection>

      {/* ── Voice Channels ── */}
      <ChannelSection label="Voice Channels">
        {voiceChannels.map((ch) => (
          <ChannelLink
            key={ch.id}
            channel={ch}
            active={pathname === `/hub/channel/${ch.id}`}
            onEdit={hasPerm(P.EDIT_OTHER_CHANNELS) ? () => openEdit(ch) : undefined}
          />
        ))}
        {hasPerm(P.CREATE_VOICE_CHANNELS) && (
          <AddButton label="Add Channel" onClick={() => openCreate("voice")} />
        )}
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
        {(hasPerm(P.INVITE_PERMANENT) || hasPerm(P.CREATE_TEMP_LINKS)) && (
          <li>
            <button className="flex w-full items-center gap-1.5 px-2 py-1 text-xs text-sidebar-foreground/40 transition-colors hover:text-sidebar-foreground/70">
              <UserPlusIcon className="h-3 w-3" />
              <span>Add Teammates</span>
            </button>
          </li>
        )}
      </ChannelSection>

      {/* ── Channel editor dialog ── */}
      <ChannelEditor
        open={editorOpen}
        onOpenChange={setEditorOpen}
        mode={editorMode}
        channelType={editorType}
        isDefault={editingChannel?.type === "text" && editingChannel?.position === 0}
        initial={
          editingChannel
            ? {
                name: editingChannel.name,
                iconId: editingChannel.iconId,
                iconColor: editingChannel.iconColor,
                iconImage: editingChannel.iconImage,
              }
            : undefined
        }
        allGroups={allGroups}
        onSave={handleSave}
        onDelete={
          editingChannel
            ? async () => { await deleteChannel(editingChannel.id) }
            : undefined
        }
      />
    </>
  )
}
