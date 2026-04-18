"use client";

import { UserPlusIcon } from "lucide-react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { useMembers, usePresence } from "@/hooks/use-members";
import { usePermissions, P } from "@/hooks/use-permissions";
import { MemberCard } from "./member-card";
import type { Member, MemberGroup } from "@/hooks/use-members";

export function MemberSidebar() {
  usePresence();
  const { members, loading, refetch } = useMembers();
  const { has } = usePermissions();
  const canInvite = has(P.INVITE_PERMANENT) || has(P.CREATE_TEMP_LINKS);

  const online = members.filter((m) => m.is_online);
  const offline = members.filter((m) => !m.is_online);

  const allGroups: MemberGroup[] = [];
  const seen = new Set<string>();
  for (const m of members) {
    for (const g of m.groups) {
      if (!seen.has(g.id)) {
        seen.add(g.id);
        allGroups.push(g);
      }
    }
  }

  return (
    <aside className="flex w-56 shrink-0 flex-col rounded-2xl bg-sidebar">
      <div className="flex h-10 items-center px-4">
        <span className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Members — {members.length}
        </span>
      </div>

      <ScrollArea className="flex-1">
        <div className="px-2 pb-2">
          {loading ? (
            <div className="space-y-2 p-2">
              {[...Array(3)].map((_, i) => (
                <div key={i} className="flex items-center gap-2">
                  <Skeleton className="h-7 w-7 rounded-md" />
                  <Skeleton className="h-4 w-24" />
                </div>
              ))}
            </div>
          ) : (
            <>
              {online.length > 0 && (
                <MemberGroupSection label={`Online — ${online.length}`}>
                  {online.map((member) => (
                    <MemberItem
                      key={member.user_id}
                      member={member}
                      allGroups={allGroups}
                      onGroupsChanged={refetch}
                    />
                  ))}
                </MemberGroupSection>
              )}

              {offline.length > 0 && (
                <MemberGroupSection label={`Offline — ${offline.length}`}>
                  {offline.map((member) => (
                    <MemberItem
                      key={member.user_id}
                      member={member}
                      allGroups={allGroups}
                      onGroupsChanged={refetch}
                    />
                  ))}
                </MemberGroupSection>
              )}
            </>
          )}
        </div>
      </ScrollArea>

      {/* Add Teammates */}
      {canInvite && (
        <div className="border-t border-border/50 p-2">
          <Button
            variant="ghost"
            size="sm"
            className="w-full justify-start gap-2 text-xs text-muted-foreground hover:text-foreground"
          >
            <UserPlusIcon className="h-3.5 w-3.5" />
            Add Teammates
          </Button>
        </div>
      )}
    </aside>
  );
}

function MemberGroupSection({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="mb-2">
      <div className="mb-1 px-2 pt-1.5 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
      <div className="space-y-0.5">{children}</div>
    </div>
  );
}

function MemberItem({
  member,
  allGroups,
  onGroupsChanged,
}: {
  member: Member;
  allGroups: MemberGroup[];
  onGroupsChanged: () => void;
}) {
  const topGroup = member.groups.find((g) => g.name !== "everyone");

  return (
    <MemberCard
      member={member}
      allGroups={allGroups}
      onGroupsChanged={onGroupsChanged}
    >
      <button className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-muted">
        <div className="relative">
          <Avatar className="h-7 w-7">
            {member.avatar_url && <AvatarImage src={member.avatar_url} />}
            <AvatarFallback
              className={`text-[10px] font-medium ${
                member.is_online
                  ? "bg-primary/15 text-primary"
                  : "bg-muted text-muted-foreground"
              }`}
            >
              {member.display_name.charAt(0).toUpperCase()}
            </AvatarFallback>
          </Avatar>
          <span
            className={`absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-card ${
              member.is_online ? "bg-emerald-500" : "bg-zinc-400"
            }`}
          />
        </div>
        <div className="min-w-0 flex-1">
          <div
            className={`truncate text-[13px] ${
              member.is_online ? "text-foreground" : "text-muted-foreground"
            }`}
          >
            {member.display_name}
          </div>
        </div>
        {topGroup && (
          <Badge
            variant="outline"
            className="shrink-0 text-[10px] px-1 py-0"
            style={
              topGroup.color
                ? {
                    backgroundColor: `${topGroup.color}20`,
                    borderColor: `${topGroup.color}40`,
                    color: topGroup.color,
                  }
                : undefined
            }
          >
            {topGroup.name}
          </Badge>
        )}
      </button>
    </MemberCard>
  );
}
