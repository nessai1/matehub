"use client";

import { useCallback, useState } from "react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { CheckIcon, PlusIcon, XIcon, ClockIcon } from "lucide-react";
import { useAuth } from "@/lib/auth";
import type { Member, MemberGroup } from "@/hooks/use-members";

const HUB_API = process.env.NEXT_PUBLIC_HUB_API_URL || "http://localhost:3002";

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
  const [groupPickerOpen, setGroupPickerOpen] = useState(false);
  const isAdmin = session?.token?.startsWith("dev-alice"); // TODO: check from JWT groups

  const memberGroupIds = new Set(member.groups.map((g) => g.id));

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

  const formatLastSeen = (iso: string | null) => {
    if (!iso) return "Never";
    const d = new Date(iso);
    const now = new Date();
    const diffMs = now.getTime() - d.getTime();
    const diffMin = Math.floor(diffMs / 60000);
    if (diffMin < 1) return "Just now";
    if (diffMin < 60) return `${diffMin}m ago`;
    const diffH = Math.floor(diffMin / 60);
    if (diffH < 24) return `${diffH}h ago`;
    return d.toLocaleDateString();
  };

  const formatExpiry = (iso: string | null) => {
    if (!iso) return null;
    const d = new Date(iso);
    const now = new Date();
    const diffMs = d.getTime() - now.getTime();
    if (diffMs <= 0) return "Expired";
    const diffH = Math.floor(diffMs / 3600000);
    const diffMin = Math.floor((diffMs % 3600000) / 60000);
    if (diffH > 0) return `${diffH}h ${diffMin}m left`;
    return `${diffMin}m left`;
  };

  return (
    <Popover>
      <PopoverTrigger asChild>{children}</PopoverTrigger>
      <PopoverContent side="left" align="start" className="w-72 p-0">
        {/* Header with avatar */}
        <div className="p-4">
          <div className="flex items-center gap-3">
            <div className="relative">
              <Avatar className="h-12 w-12">
                {member.avatar_url && (
                  <AvatarImage src={member.avatar_url} />
                )}
                <AvatarFallback className="bg-primary/15 text-lg font-semibold text-primary">
                  {member.display_name.charAt(0).toUpperCase()}
                </AvatarFallback>
              </Avatar>
              <span
                className={`absolute -bottom-0.5 -right-0.5 h-3 w-3 rounded-full border-2 border-popover ${
                  member.is_online ? "bg-emerald-500" : "bg-zinc-500"
                }`}
              />
            </div>
            <div className="min-w-0 flex-1">
              <div className="truncate font-semibold">
                {member.display_name}
              </div>
              <div className="truncate text-xs text-muted-foreground">
                @{member.username}
              </div>
            </div>
          </div>

          {/* Status line */}
          <div className="mt-3 flex items-center gap-1.5 text-xs text-muted-foreground">
            {member.is_online ? (
              <span className="text-emerald-500">Online</span>
            ) : (
              <>
                <ClockIcon className="h-3 w-3" />
                Last seen {formatLastSeen(member.last_seen_at)}
              </>
            )}
          </div>

          {/* Temp user expiry */}
          {member.user_type === "temp" && member.expires_at && (
            <div className="mt-1 flex items-center gap-1.5 text-xs text-amber-500">
              <ClockIcon className="h-3 w-3" />
              Temporary &middot; {formatExpiry(member.expires_at)}
            </div>
          )}
        </div>

        <Separator />

        {/* Groups */}
        <div className="p-3">
          <div className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Groups
          </div>
          <div className="flex flex-wrap gap-1.5">
            {member.groups.map((g) => (
              <Badge
                key={g.id}
                variant="outline"
                className="gap-1 text-xs"
                style={
                  g.color
                    ? {
                        backgroundColor: `${g.color}20`,
                        borderColor: `${g.color}40`,
                        color: g.color,
                      }
                    : undefined
                }
              >
                {g.name}
                {isAdmin && g.name !== "everyone" && (
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      toggleGroup(g.id, false);
                    }}
                    className="ml-0.5 rounded-sm opacity-60 hover:opacity-100"
                  >
                    <XIcon className="h-3 w-3" />
                  </button>
                )}
              </Badge>
            ))}

            {/* Add group button (admin only) */}
            {isAdmin && (
              <Popover
                open={groupPickerOpen}
                onOpenChange={setGroupPickerOpen}
              >
                <PopoverTrigger asChild>
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-6 gap-1 px-2 text-xs"
                  >
                    <PlusIcon className="h-3 w-3" />
                  </Button>
                </PopoverTrigger>
                <PopoverContent className="w-52 p-0" side="bottom">
                  <Command>
                    <CommandInput placeholder="Search groups..." />
                    <CommandList>
                      <CommandEmpty>No groups found</CommandEmpty>
                      <CommandGroup>
                        {allGroups.map((g) => {
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
                                style={{
                                  backgroundColor: g.color || "#666",
                                }}
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
        </div>
      </PopoverContent>
    </Popover>
  );
}
