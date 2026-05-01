import { useCallback, useState } from "react"
import { useChannels } from "@/hooks/use-channels"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar"
import {
  HashIcon,
  MicIcon,
  MicOffIcon,
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
import { useVideoCall } from "@/contexts/video-call-context"
import { useHubSelection } from "@/contexts/hub-selection-context"
import { useVoiceOccupancy } from "@/hooks/use-voice-occupancy"
import { useVoiceMute } from "@/hooks/use-voice-mute"
import { useUnreadCounts } from "@/contexts/chat-context"
import { useAddTeammates } from "@/contexts/add-teammates-context"
import { ChannelEditor, type ChannelEditorData } from "@/components/hub/channel-editor"
import { t } from "@/i18n"

export type ChannelType = "text" | "voice" | "stage" | "dm"

export interface Channel {
  id: string
  name: string
  type: ChannelType
  position: number
  iconColor?: string | null
  iconImage?: string | null
  iconId?: string
  /** Stringified user ids — present only for type === "dm". */
  participants?: string[]
}

const defaultIcons: Record<ChannelType, React.ElementType> = {
  text: HashIcon,
  voice: MicIcon,
  stage: RadioIcon,
  // DM channels render as the peer's avatar in their own components
  // (chat-workspace, member-card popover); this fallback is for the
  // generic ChannelIcon path and should rarely be hit.
  dm: MessageSquareIcon,
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

export function ChannelIcon({ channel, className }: { channel: Channel; className?: string }) {
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
  onSelect,
  onEdit,
  unread,
}: {
  channel: Channel
  active: boolean
  onSelect: () => void
  onEdit?: () => void
  unread?: number
}) {
  const showBadge = !active && unread != null && unread > 0
  return (
    <li>
      <div
        className={cn(
          "group/ch flex items-center rounded-md transition-all",
          active
            ? "border-l-[3px] border-sidebar-primary bg-sidebar-accent font-semibold text-sidebar-foreground"
            : unread && unread > 0
            ? "border-l-[3px] border-transparent font-semibold text-sidebar-foreground hover:bg-sidebar-accent/50"
            : "border-l-[3px] border-transparent text-sidebar-foreground/60 hover:bg-sidebar-accent/50 hover:text-sidebar-foreground",
        )}
      >
        <button
          type="button"
          onClick={onSelect}
          className="flex min-w-0 flex-1 items-center gap-2 px-2 py-1.5 text-left text-sm"
        >
          <ChannelIcon channel={channel} />
          <span className="truncate">{channel.name}</span>
        </button>
        {showBadge && (
          <span
            className="mr-1 inline-flex h-4 min-w-[16px] items-center justify-center rounded-full bg-sidebar-primary px-1 font-mono text-[10px] font-bold leading-none text-sidebar-primary-foreground"
            aria-label={`${unread} unread`}
          >
            {unread! > 99 ? "99+" : unread}
          </span>
        )}
        {onEdit && (
          <button
            onClick={(e) => {
              e.stopPropagation()
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

// ── Voice participants (shown under the active voice channel) ───

function VoiceParticipantsRow({
  userId,
  displayName,
  avatarUrl,
  isSelf,
  isSpeaking,
  isMicMuted,
}: {
  userId: string
  displayName: string
  avatarUrl?: string | null
  isSelf: boolean
  isSpeaking: boolean
  isMicMuted: boolean
}) {
  return (
    <li>
      <div className="flex items-center gap-2 rounded-md py-1 pl-8 pr-2 text-xs text-sidebar-foreground/70">
        {/* Speaking indicator: a static green ring + soft halo that fades
            in and out via transition-shadow. No continuous pulse — it
            just appears when speech starts and dissolves when it stops.
            Transitioning box-shadow gives the entrance/exit smoothness
            for free; both directions share the duration. */}
        <Avatar
          size="sm"
          className={cn(
            "h-4 w-4 transition-shadow duration-200 ease-out",
            isSpeaking &&
              "shadow-[0_0_0_2px_rgba(52,211,153,0.9),0_0_8px_rgba(52,211,153,0.45)]",
          )}
        >
          {avatarUrl && <AvatarImage src={avatarUrl} />}
          <AvatarFallback className="text-[8px]">
            {displayName.charAt(0).toUpperCase()}
          </AvatarFallback>
        </Avatar>
        <span className="truncate">
          {displayName}
          {isSelf && <span className="ml-1 text-[10px] text-sidebar-foreground/30">(You)</span>}
        </span>
        {isMicMuted && (
          <MicOffIcon className="ml-auto h-3 w-3 text-red-400/70" />
        )}
        {!isMicMuted && userId /* hush "unused" */ && null}
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
  const { session } = useAuth()
  const { members } = useMembers()
  const { has: hasPerm } = usePermissions()
  const {
    activeVoiceChannelId,
    joinVoice,
    participants: voiceParticipants,
    isMicEnabled: selfMicEnabled,
  } = useVideoCall()
  const { selectedTextChannelId, selectTextChannel } = useHubSelection()
  const occupancy = useVoiceOccupancy()
  const muteState = useVoiceMute()
  const unreadCounts = useUnreadCounts()
  const { open: openAddTeammates } = useAddTeammates()

  // Text and voice selections are independent — both get highlighted
  // concurrently when the user is in a call AND reading a text channel.
  const isTextActive = (id: string) => selectedTextChannelId === id
  const isVoiceActive = (id: string) => activeVoiceChannelId === id

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
    participants: ch.participants,
  }))

  // ChannelEditor only handles user-creatable kinds — DMs are minted via the
  // member-card flow, not the editor — so use a narrower type here.
  type EditableChannelType = Exclude<ChannelType, "dm">
  const [editorOpen, setEditorOpen] = useState(false)
  const [editorMode, setEditorMode] = useState<"create" | "edit">("create")
  const [editorType, setEditorType] = useState<EditableChannelType>("text")
  const [editingChannel, setEditingChannel] = useState<Channel | null>(null)

  const textChannels = channels.filter((c) => c.type === "text")
  const voiceChannels = channels.filter((c) => c.type === "voice" || c.type === "stage")
  const dmChannels = channels.filter((c) => c.type === "dm")

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

  const openCreate = (type: EditableChannelType) => {
    setEditorMode("create")
    setEditorType(type)
    setEditingChannel(null)
    setEditorOpen(true)
  }

  const openEdit = (channel: Channel) => {
    if (channel.type === "dm") return // DMs aren't editable via this dialog
    setEditorMode("edit")
    setEditorType(channel.type)
    setEditingChannel(channel)
    setEditorOpen(true)
  }

  const handleSave = useCallback(async (data: ChannelEditorData) => {
    let targetId: string | null = null
    let createdType: ChannelType | null = null

    if (editorMode === "create") {
      const ch = await createChannel({
        name: data.name,
        type: data.type,
        icon_id: data.iconId,
        icon_color: data.iconColor,
      })
      targetId = ch?.id ?? null
      createdType = ch?.type ?? null
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

    // Switch to the just-created text channel. Voice/stage need an explicit
    // join action, not automatic selection.
    if (editorMode === "create" && createdType === "text" && targetId) {
      selectTextChannel(targetId)
    }
  }, [editorMode, editingChannel, createChannel, updateChannel, uploadIcon, selectTextChannel])

  return (
    <>
      {/* ── Text Channels ── */}
      <ChannelSection label={t("Channels")}>
        {textChannels.map((ch) => (
          <ChannelLink
            key={ch.id}
            channel={ch}
            active={isTextActive(ch.id)}
            onSelect={() => selectTextChannel(ch.id)}
            onEdit={hasPerm(P.EDIT_OTHER_CHANNELS) ? () => openEdit(ch) : undefined}
            unread={unreadCounts.get(ch.id)?.unread}
          />
        ))}
        {hasPerm(P.CREATE_TEXT_CHANNELS) && (
          <AddButton label={t("Add Channel")} onClick={() => openCreate("text")} />
        )}
      </ChannelSection>

      {/* ── Voice Channels ── */}
      <ChannelSection label={t("Voice Channels")}>
        {voiceChannels.map((ch) => {
          // Roster is occupancy-driven — list everyone in this voice channel,
          // whether or not *we* are in it. Speaking/mic-muted state only
          // exists for the channel we ourselves are connected to (SDK gives
          // it via voiceParticipants); for other channels those stay at
          // their defaults.
          const isActiveCall = activeVoiceChannelId === ch.id
          const inThisChannel = members.filter(
            (m) => occupancy.get(m.user_id) === ch.id,
          )
          const sdkByUserId = new Map(
            voiceParticipants.map((p) => [p.userId, p]),
          )
          return (
            <div key={ch.id}>
              <ChannelLink
                channel={ch}
                active={isVoiceActive(ch.id)}
                onSelect={() => void joinVoice(ch.id)}
                onEdit={hasPerm(P.EDIT_OTHER_CHANNELS) ? () => openEdit(ch) : undefined}
              />
              {inThisChannel.length > 0 && (
                <ul className="flex flex-col gap-0.5 py-0.5">
                  {inThisChannel.map((m) => {
                    const isSelf = m.user_id === session?.userId
                    // SDK keys participants by the SFU `user_id` (display
                    // name / username). Try both keys to find mic-muted /
                    // speaking state when we're the one in this call.
                    const sdk = isActiveCall
                      ? sdkByUserId.get(m.username) ?? sdkByUserId.get(m.user_id)
                      : undefined
                    return (
                      <VoiceParticipantsRow
                        key={m.user_id}
                        userId={m.user_id}
                        displayName={m.display_name}
                        avatarUrl={m.avatar_url}
                        isSelf={isSelf}
                        isSpeaking={sdk?.isSpeaking ?? false}
                        // Priority order:
                        //   1. Self → local SDK state (no roundtrip lag).
                        //   2. Same call as us → SDK state from peer
                        //      (sub-second updates via SFU WS).
                        //   3. Other call → hub-wide voice-mute store
                        //      (presence-WS, ~tens of ms lag).
                        // Default both muted matches video service's
                        // Participant defaults — keeps cold start safe.
                        isMicMuted={
                          isSelf
                            ? !selfMicEnabled
                            : sdk?.isMicMuted ??
                              muteState.get(m.user_id)?.audioMuted ??
                              true
                        }
                      />
                    )
                  })}
                </ul>
              )}
            </div>
          )
        })}
        {hasPerm(P.CREATE_VOICE_CHANNELS) && (
          <AddButton label={t("Add Channel")} onClick={() => openCreate("voice")} />
        )}
      </ChannelSection>

      {/* ── Direct Messages ──
          Only show DMs the user has actually opened. The list renders
          channel-by-channel — a member with no DM history doesn't appear
          here, by design (start one via the right-side member card). */}
      {dmChannels.length > 0 && (
        <ChannelSection label={t("Direct Messages")}>
          {dmChannels.map((dm) => {
            const peerId = dm.participants?.find((id) => id !== session?.userId)
            const peer = peerId ? members.find((m) => m.user_id === peerId) : undefined
            const displayName = peer?.display_name ?? peerId ?? "Unknown"
            const active = isTextActive(dm.id)
            const unread = unreadCounts.get(dm.id)?.unread
            const showBadge = !active && unread != null && unread > 0
            return (
              <li key={dm.id}>
                <button
                  type="button"
                  onClick={() => selectTextChannel(dm.id)}
                  className={cn(
                    "group/dm flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors",
                    active
                      ? "bg-sidebar-accent font-semibold text-sidebar-foreground"
                      : "text-sidebar-foreground/60 hover:bg-sidebar-accent/50 hover:text-sidebar-foreground",
                  )}
                >
                  <div className="relative shrink-0">
                    <Avatar size="sm">
                      {peer?.avatar_url && <AvatarImage src={peer.avatar_url} />}
                      <AvatarFallback
                        className={cn(
                          "text-[10px] font-medium",
                          peer?.is_online
                            ? "bg-primary/15 text-primary"
                            : "bg-muted text-muted-foreground",
                        )}
                      >
                        {displayName.charAt(0).toUpperCase()}
                      </AvatarFallback>
                    </Avatar>
                    <span
                      className={cn(
                        "absolute -bottom-0.5 -right-0.5 h-2 w-2 rounded-full border border-sidebar",
                        peer?.is_online ? "bg-emerald-500" : "bg-zinc-500",
                      )}
                    />
                  </div>
                  <span className="flex-1 truncate text-left">{displayName}</span>
                  {showBadge && (
                    <span
                      className="inline-flex h-4 min-w-[16px] items-center justify-center rounded-full bg-sidebar-primary px-1 font-mono text-[10px] font-bold leading-none text-sidebar-primary-foreground"
                      aria-label={`${unread} unread`}
                    >
                      {unread! > 99 ? "99+" : unread}
                    </span>
                  )}
                </button>
              </li>
            )
          })}
          {(hasPerm(P.INVITE_PERMANENT) || hasPerm(P.CREATE_TEMP_LINKS)) && (
            <li>
              <button
                onClick={openAddTeammates}
                className="flex w-full items-center gap-1.5 px-2 py-1 text-xs text-sidebar-foreground/40 transition-colors hover:text-sidebar-foreground/70"
              >
                <UserPlusIcon className="h-3 w-3" />
                <span>{t("Add Teammates")}</span>
              </button>
            </li>
          )}
        </ChannelSection>
      )}

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
