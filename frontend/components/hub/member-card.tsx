import { useCallback, useEffect, useState } from "react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Button } from "@/components/ui/button";
import {
  CheckIcon,
  PlusIcon,
  XIcon,
  PhoneIcon,
  SendHorizontalIcon,
  UserMinusIcon,
} from "lucide-react";
import { useAuth } from "@/lib/auth";
import { usePermissions, P } from "@/hooks/use-permissions";
import { isMemberActive, type Member, type MemberGroup } from "@/hooks/use-members";
import { useChannels } from "@/hooks/use-channels";
import { useIncomingCall } from "@/contexts/incoming-call-context";
import { useHubSelection } from "@/contexts/hub-selection-context";
import { t } from "@/i18n";

const HUB_API = import.meta.env.VITE_HUB_API_URL || "http://localhost:3002";

interface AllGroup {
  id: string;
  name: string;
  color: string | null;
  position: number;
}

interface MemberCardProps {
  member: Member;
  allGroups: MemberGroup[];
  onGroupsChanged?: () => void;
  children: React.ReactNode;
}

export function MemberCard({
  member,
  allGroups,
  onGroupsChanged,
  children,
}: MemberCardProps) {
  const { session } = useAuth();
  const { perms, has } = usePermissions();
  const { openDm } = useChannels();
  const { startCall } = useIncomingCall();
  const { selectTextChannel } = useHubSelection();
  const [groupPickerOpen, setGroupPickerOpen] = useState(false);
  const [kickDialogOpen, setKickDialogOpen] = useState(false);
  const [kicking, setKicking] = useState(false);
  const [apiGroups, setApiGroups] = useState<AllGroup[]>([]);
  const [popoverOpen, setPopoverOpen] = useState(false);

  const canManageRoles = has(P.MANAGE_ROLES);
  const canManageMembers = has(P.MANAGE_MEMBERS);
  const isSelf = session?.userId === member.user_id;
  // Inactive = expired-temp or scrubbed account. They appear here only because
  // chat history references them; you can't ping/message/kick a ghost.
  const active = isMemberActive(member);
  const inactiveLabel = member.deleted_at
    ? t("Deleted user")
    : member.user_type === "temp"
      ? t("Guest session expired")
      : t("No longer in hub");

  const handleMessage = useCallback(async () => {
    const channel = await openDm(member.user_id);
    if (channel) {
      selectTextChannel(channel.id);
      setPopoverOpen(false);
    }
  }, [openDm, selectTextChannel, member.user_id]);

  const handleCall = useCallback(async () => {
    const channel = await openDm(member.user_id);
    if (channel) {
      void startCall(channel.id);
      setPopoverOpen(false);
    }
  }, [openDm, startCall, member.user_id]);

  // Fetch all groups from API when picker opens (includes groups with 0 members)
  const fetchApiGroups = useCallback(async () => {
    if (!session?.hubId || !session?.token) return;
    const res = await fetch(`${HUB_API}/v1/hubs/${session.hubId}/groups`, {
      headers: { Authorization: `Bearer ${session.token}` },
    });
    if (res.ok) setApiGroups(await res.json());
  }, [session?.hubId, session?.token]);

  useEffect(() => {
    if (groupPickerOpen) fetchApiGroups();
  }, [groupPickerOpen, fetchApiGroups]);

  const memberGroupIds = new Set(member.groups.map((g) => g.id));
  // Use API groups for picker (has all groups), fallback to allGroups prop
  const pickerGroups = apiGroups.length > 0 ? apiGroups : allGroups.map((g) => ({ ...g, position: 0 }));

  const toggleGroup = useCallback(
    async (groupId: string, add: boolean) => {
      if (!session) return;
      const url = `${HUB_API}/v1/hubs/${session.hubId}/groups/${groupId}/members/${member.user_id}`;
      await fetch(url, {
        method: add ? "POST" : "DELETE",
        headers: { Authorization: `Bearer ${session.token}` },
      });
      onGroupsChanged?.();
    },
    [session, member.user_id, onGroupsChanged],
  );

  const kickMember = useCallback(async () => {
    if (!session) return;
    setKicking(true);
    try {
      await fetch(
        `${HUB_API}/v1/hubs/${session.hubId}/members/${member.user_id}`,
        {
          method: "DELETE",
          headers: { Authorization: `Bearer ${session.token}` },
        },
      );
      onGroupsChanged?.();
      setKickDialogOpen(false);
    } catch {
      // ignore
    } finally {
      setKicking(false);
    }
  }, [session, member.user_id, onGroupsChanged]);

  /** Same-day → HH:MM, otherwise "DD MMM, HH:MM". Mirrors the helper in
   *  member-sidebar.tsx; kept inline here so the popup is independent of
   *  the sidebar component. */
  const formatExpiry = (iso: string): string => {
    try {
      const d = new Date(iso);
      const now = new Date();
      const sameDay =
        d.getFullYear() === now.getFullYear() &&
        d.getMonth() === now.getMonth() &&
        d.getDate() === now.getDate();
      return d.toLocaleString(
        undefined,
        sameDay
          ? { hour: "2-digit", minute: "2-digit" }
          : {
              day: "numeric",
              month: "short",
              hour: "2-digit",
              minute: "2-digit",
            },
      );
    } catch {
      return iso;
    }
  };

  const formatLastSeen = (iso: string | null) => {
    if (!iso) return t("Never");
    const d = new Date(iso);
    const diffMin = Math.floor((Date.now() - d.getTime()) / 60000);
    if (diffMin < 1) return t("Just now");
    if (diffMin < 60) return t("%dm ago", diffMin);
    const diffH = Math.floor(diffMin / 60);
    if (diffH < 24) return t("%dh ago", diffH);
    return d.toLocaleDateString();
  };

  return (
    <>
      <Popover open={popoverOpen} onOpenChange={setPopoverOpen}>
        <PopoverTrigger asChild>{children}</PopoverTrigger>
        <PopoverContent
          side="right"
          align="center"
          className="w-64 overflow-hidden rounded-2xl border-0 bg-popover p-0 shadow-2xl"
        >
          <div className="flex flex-col gap-4 p-5">
            {/* Avatar + info */}
            <div className="flex flex-col items-center">
              <Avatar className="h-24 w-24">
                {member.avatar_url && <AvatarImage src={member.avatar_url} />}
                <AvatarFallback className="bg-muted text-2xl font-bold text-muted-foreground">
                  {member.display_name.charAt(0).toUpperCase()}
                </AvatarFallback>
              </Avatar>

              {!active ? (
                <span className="mt-2 rounded-full bg-muted px-2.5 py-0.5 text-[10px] italic text-muted-foreground">
                  {inactiveLabel}
                </span>
              ) : member.is_online ? (
                <span className="mt-2 rounded-full bg-emerald-500/15 px-2.5 py-0.5 text-[10px] font-medium text-emerald-500">
                  {t("online")}
                </span>
              ) : (
                <span className="mt-2 rounded-full bg-muted px-2.5 py-0.5 text-[10px] text-muted-foreground">
                  {formatLastSeen(member.last_seen_at)}
                </span>
              )}

              <h3 className="mt-2 text-base font-semibold text-foreground">
                {member.display_name}
              </h3>
              {!member.deleted_at && (
                <p className="text-xs text-muted-foreground">@{member.username}</p>
              )}

              {member.user_type === "temp" && active && (
                <span className="mt-0.5 rounded-full bg-amber-500/15 px-2.5 py-0.5 text-[10px] text-amber-500">
                  {member.expires_at
                    ? t("Guest until %s", formatExpiry(member.expires_at))
                    : t("Temporary")}
                </span>
              )}
            </div>

            {/* Groups */}
            <div className="flex flex-wrap items-center gap-1.5">
              {member.groups.map((g) => {
                // Can't remove:
                //   - "everyone" (the default group) on anyone
                //   - "admin" on a non-creator (only creator can demote admins)
                //   - any group on yourself (use the profile dialog instead;
                //     also prevents the creator from demoting themselves and
                //     orphaning the hub)
                const isProtected =
                  g.name.toLowerCase() === "everyone" ||
                  (g.name.toLowerCase() === "admin" && perms?.is_creator === false) ||
                  isSelf;
                const canRemove = canManageRoles && !isProtected && active;
                return (
                  <Badge
                    key={g.id}
                    variant="outline"
                    className="gap-1 text-[11px]"
                    style={
                      g.color
                        ? {
                            backgroundColor: `${g.color}15`,
                            borderColor: `${g.color}30`,
                            color: g.color,
                          }
                        : undefined
                    }
                  >
                    {g.name}
                    {canRemove && (
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          toggleGroup(g.id, false);
                        }}
                        className="ml-0.5 rounded-sm opacity-50 hover:opacity-100"
                      >
                        <XIcon className="h-2.5 w-2.5" />
                      </button>
                    )}
                  </Badge>
                );
              })}
              {canManageRoles && !isSelf && active && (
                <Popover open={groupPickerOpen} onOpenChange={setGroupPickerOpen}>
                  <PopoverTrigger asChild>
                    <button className="flex h-5 w-5 items-center justify-center rounded border border-border text-muted-foreground hover:text-foreground">
                      <PlusIcon className="h-3 w-3" />
                    </button>
                  </PopoverTrigger>
                  <PopoverContent className="w-52 p-0" side="bottom">
                    <Command>
                      <CommandInput placeholder={t("Search groups...")} />
                      <CommandList>
                        <CommandEmpty>{t("No groups found")}</CommandEmpty>
                        <CommandGroup>
                          {pickerGroups.map((g) => {
                            const assigned = memberGroupIds.has(g.id);
                            return (
                              <CommandItem
                                key={g.id}
                                onSelect={() => {
                                  toggleGroup(g.id, !assigned);
                                  setGroupPickerOpen(false);
                                }}
                              >
                                <div
                                  className="mr-2 h-2 w-2 rounded-full"
                                  style={{ backgroundColor: g.color || "#666" }}
                                />
                                {g.name}
                                {assigned && (
                                  <CheckIcon className="ml-auto h-3.5 w-3.5 text-primary" />
                                )}
                              </CommandItem>
                            );
                          })}
                        </CommandGroup>
                      </CommandList>
                    </Command>
                  </PopoverContent>
                </Popover>
              )}
            </div>

            {/* Actions */}
            {!isSelf && active && (
              <div className="flex items-center justify-center gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  className="h-9 w-9 rounded-full p-0"
                  onClick={handleCall}
                >
                  <PhoneIcon className="h-4 w-4" />
                </Button>

                <Button
                  size="sm"
                  className="h-9 gap-1.5 rounded-full bg-blue-600 px-5 text-xs text-white hover:bg-blue-500"
                  onClick={handleMessage}
                >
                  <SendHorizontalIcon className="h-3.5 w-3.5" />
                  {t("Message")}
                </Button>

                {canManageMembers && (
                  <Button
                    size="sm"
                    variant="outline"
                    className="h-9 w-9 rounded-full border-red-200 p-0 text-red-500 hover:bg-red-50 dark:border-red-900 dark:text-red-400 dark:hover:bg-red-950"
                    onClick={() => setKickDialogOpen(true)}
                  >
                    <UserMinusIcon className="h-4 w-4" />
                  </Button>
                )}
              </div>
            )}
          </div>
        </PopoverContent>
      </Popover>

      {/* Kick confirmation dialog */}
      <Dialog open={kickDialogOpen} onOpenChange={(open: boolean) => setKickDialogOpen(open)}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>{t("Remove member")}</DialogTitle>
          </DialogHeader>
          <p className="text-sm text-muted-foreground">
            {t("Are you sure you want to remove %s from the hub?", member.display_name)}
          </p>
          <DialogFooter className="gap-2 sm:justify-center">
            <Button variant="outline" onClick={() => setKickDialogOpen(false)}>
              {t("Cancel")}
            </Button>
            <Button
              variant="destructive"
              onClick={kickMember}
              disabled={kicking}
            >
              {kicking ? t("Removing...") : t("Remove")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
