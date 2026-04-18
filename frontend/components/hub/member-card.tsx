"use client";

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
import type { Member, MemberGroup } from "@/hooks/use-members";

const HUB_API = process.env.NEXT_PUBLIC_HUB_API_URL || "http://localhost:3002";

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
  const [groupPickerOpen, setGroupPickerOpen] = useState(false);
  const [kickDialogOpen, setKickDialogOpen] = useState(false);
  const [kicking, setKicking] = useState(false);
  const [apiGroups, setApiGroups] = useState<AllGroup[]>([]);

  const canManageRoles = has(P.MANAGE_ROLES);
  const canManageMembers = has(P.MANAGE_MEMBERS);
  const isSelf = session?.userId === member.user_id;

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

  const formatLastSeen = (iso: string | null) => {
    if (!iso) return "Never";
    const d = new Date(iso);
    const diffMin = Math.floor((Date.now() - d.getTime()) / 60000);
    if (diffMin < 1) return "Just now";
    if (diffMin < 60) return `${diffMin}m ago`;
    const diffH = Math.floor(diffMin / 60);
    if (diffH < 24) return `${diffH}h ago`;
    return d.toLocaleDateString();
  };

  return (
    <>
      <Popover>
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

              {member.is_online ? (
                <span className="mt-2 rounded-full bg-emerald-500/15 px-2.5 py-0.5 text-[10px] font-medium text-emerald-500">
                  online
                </span>
              ) : (
                <span className="mt-2 rounded-full bg-muted px-2.5 py-0.5 text-[10px] text-muted-foreground">
                  {formatLastSeen(member.last_seen_at)}
                </span>
              )}

              <h3 className="mt-2 text-base font-semibold text-foreground">
                {member.display_name}
              </h3>
              <p className="text-xs text-muted-foreground">@{member.username}</p>

              {member.user_type === "temp" && (
                <span className="mt-0.5 rounded-full bg-amber-500/15 px-2.5 py-0.5 text-[10px] text-amber-500">
                  Temporary
                </span>
              )}
            </div>

            {/* Groups */}
            <div className="flex flex-wrap items-center gap-1.5">
              {member.groups.map((g) => {
                // Can't remove: everyone (is_default), admin on creator
                const isProtected =
                  g.name.toLowerCase() === "everyone" ||
                  (g.name.toLowerCase() === "admin" && perms?.is_creator === false);
                const canRemove = canManageRoles && !isProtected;
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
              {canManageRoles && (
                <Popover open={groupPickerOpen} onOpenChange={setGroupPickerOpen}>
                  <PopoverTrigger asChild>
                    <button className="flex h-5 w-5 items-center justify-center rounded border border-border text-muted-foreground hover:text-foreground">
                      <PlusIcon className="h-3 w-3" />
                    </button>
                  </PopoverTrigger>
                  <PopoverContent className="w-52 p-0" side="bottom">
                    <Command>
                      <CommandInput placeholder="Search groups..." />
                      <CommandList>
                        <CommandEmpty>No groups found</CommandEmpty>
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
            {!isSelf && (
              <div className="flex items-center justify-center gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  className="h-9 w-9 rounded-full p-0"
                >
                  <PhoneIcon className="h-4 w-4" />
                </Button>

                <Button
                  size="sm"
                  className="h-9 gap-1.5 rounded-full bg-blue-600 px-5 text-xs text-white hover:bg-blue-500"
                >
                  <SendHorizontalIcon className="h-3.5 w-3.5" />
                  Message
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
            <DialogTitle>Remove member</DialogTitle>
          </DialogHeader>
          <p className="text-sm text-muted-foreground">
            Are you sure you want to remove <strong>{member.display_name}</strong> from the hub?
          </p>
          <DialogFooter className="gap-2 sm:justify-center">
            <Button variant="outline" onClick={() => setKickDialogOpen(false)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={kickMember}
              disabled={kicking}
            >
              {kicking ? "Removing..." : "Remove"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
