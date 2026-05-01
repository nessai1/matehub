import { useState } from "react";
import { ClockIcon, LinkIcon, Trash2Icon, UserPlusIcon } from "lucide-react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { useAuth } from "@/lib/auth";
import { useMembers, usePresence } from "@/hooks/use-members";
import {
  deletePendingInvite,
  usePendingInvites,
} from "@/hooks/use-pending-invites";
import { usePermissions, P } from "@/hooks/use-permissions";
import { useAddTeammates } from "@/contexts/add-teammates-context";
import { useMemberSidebar } from "@/contexts/member-sidebar-context";
import { cn } from "@/lib/utils";
import { MemberCard } from "./member-card";
import type { Member, MemberGroup } from "@/hooks/use-members";
import type { PendingInvite } from "@/hooks/use-pending-invites";
import { t } from "@/i18n";

export function MemberSidebar() {
  usePresence();
  const { members, loading, refetch } = useMembers();
  const { has } = usePermissions();
  const { open: openAddTeammates } = useAddTeammates();
  const { open } = useMemberSidebar();
  const canInvite = has(P.INVITE_PERMANENT) || has(P.CREATE_TEMP_LINKS);
  // Pending invites are admin/inviter-only signal — don't leak names of people
  // who haven't joined to ordinary members.
  const { invites: pending } = usePendingInvites();
  const showPending = canInvite && pending.length > 0;

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
    <aside
      data-state={open ? "expanded" : "collapsed"}
      className={cn(
        "flex shrink-0 flex-col overflow-hidden rounded-2xl bg-sidebar transition-[width] duration-200 ease-linear",
        open ? "w-56" : "w-14",
      )}
    >
      {open && (
        <div className="flex h-10 items-center px-4">
          <span className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            {t("Members")} — {members.length}
          </span>
        </div>
      )}

      <ScrollArea className="flex-1">
        <div className={cn("pb-2", open ? "px-2" : "px-1.5")}>
          {loading ? (
            <div className="space-y-2 p-2">
              {[...Array(3)].map((_, i) => (
                <div key={i} className="flex items-center gap-2">
                  <Skeleton className="h-7 w-7 rounded-md" />
                  {open && <Skeleton className="h-4 w-24" />}
                </div>
              ))}
            </div>
          ) : (
            <>
              {online.length > 0 && (
                <MemberGroupSection
                  collapsed={!open}
                  label={`${t("Online")} — ${online.length}`}
                >
                  {online.map((member) => (
                    <MemberItem
                      key={member.user_id}
                      member={member}
                      allGroups={allGroups}
                      onGroupsChanged={refetch}
                      collapsed={!open}
                    />
                  ))}
                </MemberGroupSection>
              )}

              {offline.length > 0 && (
                <MemberGroupSection
                  collapsed={!open}
                  label={`${t("Offline")} — ${offline.length}`}
                >
                  {offline.map((member) => (
                    <MemberItem
                      key={member.user_id}
                      member={member}
                      allGroups={allGroups}
                      onGroupsChanged={refetch}
                      collapsed={!open}
                    />
                  ))}
                </MemberGroupSection>
              )}

              {showPending && (
                <MemberGroupSection
                  collapsed={!open}
                  label={`${t("Invited")} — ${pending.length}`}
                >
                  {pending.map((inv) => (
                    <PendingInviteItem
                      key={`${inv.kind}-${inv.id}`}
                      invite={inv}
                      collapsed={!open}
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
        <div className={cn("border-t border-border/50", open ? "p-2" : "flex justify-center p-1.5")}>
          <Button
            variant="ghost"
            size={open ? "sm" : "icon-sm"}
            onClick={openAddTeammates}
            title={open ? undefined : t("Add Teammates")}
            className={cn(
              "text-muted-foreground hover:text-foreground",
              open ? "w-full justify-start gap-2 text-xs" : "h-8 w-8",
            )}
          >
            <UserPlusIcon className="h-3.5 w-3.5" />
            {open && t("Add Teammates")}
          </Button>
        </div>
      )}
    </aside>
  );
}

function MemberGroupSection({
  label,
  children,
  collapsed,
}: {
  label: string;
  children: React.ReactNode;
  collapsed?: boolean;
}) {
  return (
    <div className="mb-2">
      {!collapsed && (
        <div className="mb-1 px-2 pt-1.5 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          {label}
        </div>
      )}
      <div className="space-y-0.5">{children}</div>
    </div>
  );
}

function PendingInviteItem({
  invite,
  collapsed,
}: {
  invite: PendingInvite;
  collapsed?: boolean;
}) {
  const { session } = useAuth();
  const { has } = usePermissions();
  const [open, setOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState("");

  const isTemp = invite.kind === "temp";
  const kindLabel = isTemp
    ? t("Temp link — not yet used")
    : t("Permanent invite — not yet accepted");

  // Authoritative check still happens server-side; this just hides the
  // button when we know the call would 403.
  const isCreator = session?.userId === invite.created_by;
  const hasPerm = isTemp
    ? has(P.CREATE_TEMP_LINKS) || has(P.MANAGE_MEMBERS)
    : has(P.INVITE_PERMANENT) || has(P.MANAGE_MEMBERS);
  const canDelete = isCreator || hasPerm;

  const onDelete = async () => {
    if (!session?.hubId || !session?.token || deleting) return;
    setError("");
    setDeleting(true);
    const status = await deletePendingInvite(session.hubId, session.token, invite);
    if (status === 204 || status === 404 || status === 410) {
      setOpen(false); // SWR refetch fires from inside deletePendingInvite
    } else if (status === 403) {
      setError(t("No permission to delete this invite"));
    } else {
      setError(`${t("Error")}: ${status}`);
    }
    setDeleting(false);
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          title={collapsed ? invite.name : undefined}
          className={cn(
            "flex w-full items-center rounded-md py-1.5 text-left transition-colors hover:bg-muted",
            collapsed ? "justify-center px-0" : "gap-2 px-2",
          )}
        >
          <div className="relative">
            <Avatar className="h-7 w-7">
              <AvatarFallback className="bg-muted/40 text-[10px] font-medium text-muted-foreground">
                <ClockIcon className="h-3.5 w-3.5" />
              </AvatarFallback>
            </Avatar>
            {isTemp && (
              <span className="absolute -bottom-0.5 -right-0.5 flex h-3 w-3 items-center justify-center rounded-full border-2 border-card bg-amber-500/80">
                <LinkIcon className="h-1.5 w-1.5 text-white" />
              </span>
            )}
          </div>
          {!collapsed && (
            <div className="min-w-0 flex-1">
              <div className="truncate text-[13px] italic text-muted-foreground">
                {invite.name}
              </div>
            </div>
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent side="left" align="start" className="w-72 p-0">
        <div className="p-3 space-y-3">
          <div className="space-y-1">
            <div className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
              {kindLabel}
            </div>
            <div className="text-sm font-medium">{invite.name}</div>
            {invite.email && (
              <div className="text-xs text-muted-foreground">{invite.email}</div>
            )}
          </div>

          <div className="space-y-1.5 border-t pt-2 text-xs">
            <Row label={t("Created")} value={formatDateTime(invite.created_at)} />
            <Row
              label={t("By")}
              value={`${invite.created_by_name} (${invite.created_by_username})`}
            />
            {invite.expires_at && (
              <Row
                label={t("Expires")}
                value={formatDateTime(invite.expires_at)}
              />
            )}
          </div>

          {error && (
            <p className="rounded border border-destructive/30 bg-destructive/10 px-2 py-1 text-xs text-destructive">
              {error}
            </p>
          )}

          {canDelete && (
            <Button
              variant="destructive"
              size="sm"
              className="w-full gap-2"
              onClick={onDelete}
              disabled={deleting}
            >
              <Trash2Icon className="h-3.5 w-3.5" />
              {deleting ? t("Deleting...") : t("Delete invite")}
            </Button>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span className="truncate text-right" title={value}>
        {value}
      </span>
    </div>
  );
}

/** Locale-aware short timestamp ("12 Mar 14:32" / "12 мар 14:32"). */
function formatDateTime(iso: string): string {
  try {
    return new Date(iso).toLocaleString(undefined, {
      day: "numeric",
      month: "short",
      hour: "2-digit",
      minute: "2-digit",
    });
  } catch {
    return iso;
  }
}

function MemberItem({
  member,
  allGroups,
  onGroupsChanged,
  collapsed,
}: {
  member: Member;
  allGroups: MemberGroup[];
  onGroupsChanged: () => void;
  collapsed?: boolean;
}) {
  const topGroup = member.groups.find((g) => g.name !== "everyone");

  return (
    <MemberCard
      member={member}
      allGroups={allGroups}
      onGroupsChanged={onGroupsChanged}
    >
      <button
        title={collapsed ? member.display_name : undefined}
        className={cn(
          "flex w-full items-center rounded-md py-1.5 text-left transition-colors hover:bg-muted",
          collapsed ? "justify-center px-0" : "gap-2 px-2",
        )}
      >
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
        {!collapsed && (
          <>
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
          </>
        )}
      </button>
    </MemberCard>
  );
}
