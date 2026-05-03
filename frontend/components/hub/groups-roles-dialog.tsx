import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import {
  PlusIcon,
  Trash2Icon,
  ChevronLeftIcon,
  ShieldIcon,
  CheckIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useAuth } from "@/lib/auth";
import { usePermissions, HUB_PERMISSION_LABELS, hasBit, P } from "@/hooks/use-permissions";
import { t } from "@/i18n";

const HUB_API = "/api/hub";

interface Group {
  id: string;
  name: string;
  color: string | null;
  position: number;
  is_default: boolean;
  hub_permissions: number;
}

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called when groups are modified (create/delete/update) so parents can refetch */
  onGroupsChanged?: () => void;
}

export function GroupsRolesDialog({ open, onOpenChange, onGroupsChanged }: Props) {
  const { session } = useAuth();
  const { perms, has } = usePermissions();
  const [groups, setGroups] = useState<Group[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const headers = useMemo(
    (): Record<string, string> => ({
      "Content-Type": "application/json",
      Authorization: `Bearer ${session?.token}`,
    }),
    [session?.token],
  );

  // Fetch groups when the dialog opens. Inline IIFE so the await chain is
  // visible to React Compiler (a separate useCallback would look like a
  // sync setState in the effect body).
  useEffect(() => {
    if (!open || !session?.hubId || !session?.token) return;
    let cancelled = false;
    const hubId = session.hubId;
    const token = session.token;
    (async () => {
      const res = await fetch(`${HUB_API}/v1/hubs/${hubId}/groups`, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (cancelled || !res.ok) return;
      const data: Group[] = await res.json();
      if (!cancelled) {
        setGroups(data.sort((a, b) => a.position - b.position));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, session?.hubId, session?.token]);

  const handleOpenChange = useCallback(
    (next: boolean) => {
      if (!next) setSelectedId(null);
      onOpenChange(next);
    },
    [onOpenChange],
  );

  const selected = groups.find((g) => g.id === selectedId) ?? null;
  const isProtected = selected?.name === "admin" || selected?.name === "everyone";
  const canManage = has(P.MANAGE_ROLES);
  const canManageSelected = canManage && selected && (perms?.is_admin || (perms?.top_position ?? Infinity) < selected.position);

  // ── Create group ──
  const createGroup = useCallback(async () => {
    if (!session?.hubId) return;
    const res = await fetch(`${HUB_API}/v1/hubs/${session.hubId}/groups`, {
      method: "POST",
      headers,
      body: JSON.stringify({ name: "New Role", color: "#6366f1" }),
    });
    if (res.ok) {
      const g: Group = (await res.json());
      setGroups((prev) => [...prev, g]);
      setSelectedId(g.id);
      onGroupsChanged?.();
    }
  }, [session, headers, onGroupsChanged]);

  // ── Update group ──
  const updateGroup = useCallback(
    async (groupId: string, data: Partial<{ name: string; color: string; hub_permissions: number }>) => {
      if (!session?.hubId) return;
      const res = await fetch(`${HUB_API}/v1/hubs/${session.hubId}/groups/${groupId}`, {
        method: "PATCH",
        headers,
        body: JSON.stringify(data),
      });
      if (res.ok) {
        const updated: Group = await res.json();
        setGroups((prev) => prev.map((g) => (g.id === groupId ? updated : g)));
        onGroupsChanged?.();
      }
    },
    [session, headers, onGroupsChanged],
  );

  // ── Delete group ──
  const deleteGroup = useCallback(
    async (groupId: string) => {
      if (!session?.hubId) return;
      const res = await fetch(`${HUB_API}/v1/hubs/${session.hubId}/groups/${groupId}`, {
        method: "DELETE",
        headers,
      });
      if (res.ok) {
        setGroups((prev) => prev.filter((g) => g.id !== groupId));
        setSelectedId(null);
        onGroupsChanged?.();
      }
    },
    [session, headers, onGroupsChanged],
  );

  // ── Toggle permission bit ──
  const toggleBit = useCallback(
    (bit: number) => {
      if (!selected || isProtected) return;
      const newBits = hasBit(selected.hub_permissions, bit)
        ? selected.hub_permissions & ~bit
        : selected.hub_permissions | bit;
      updateGroup(selected.id, { hub_permissions: newBits });
    },
    [selected, isProtected, updateGroup],
  );

  // ── Colors ──
  const colors = [
    "#E74C3C", "#E67E22", "#F1C40F", "#2ECC71",
    "#1ABC9C", "#3498DB", "#9B59B6", "#6366f1",
    "#EC4899", "#99AAB5", "#95A5A6", "#607D8B",
  ];

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-lg p-0 gap-0">
        <DialogHeader className="p-4 pb-0">
          <DialogTitle className="flex items-center gap-2">
            {selected && (
              <button onClick={() => setSelectedId(null)} className="text-muted-foreground hover:text-foreground">
                <ChevronLeftIcon className="h-4 w-4" />
              </button>
            )}
            <ShieldIcon className="h-4 w-4" />
            {selected ? selected.name : t("Groups & Roles")}
          </DialogTitle>
        </DialogHeader>

        <div className="p-4">
          {!selected ? (
            /* ── Group list ── */
            <div className="flex flex-col gap-1">
              {groups.map((g) => (
                <button
                  key={g.id}
                  onClick={() => setSelectedId(g.id)}
                  className="flex items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors hover:bg-muted"
                >
                  <div
                    className="h-3 w-3 shrink-0 rounded-full"
                    style={{ backgroundColor: g.color || "#6b7280" }}
                  />
                  <span className="flex-1 text-sm font-medium">{g.name}</span>
                  {(g.name === "admin" || g.name === "everyone") && (
                    <Badge variant="outline" className="text-[10px]">{t("protected")}</Badge>
                  )}
                </button>
              ))}

              {canManage && (
                <button
                  onClick={createGroup}
                  className="flex items-center gap-2 rounded-lg px-3 py-2 text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                >
                  <PlusIcon className="h-3.5 w-3.5" />
                  {t("Create role")}
                </button>
              )}
            </div>
          ) : (
            /* ── Group editor ── */
            <div className="flex flex-col gap-4">
              {/* Name */}
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-muted-foreground">{t("Name")}</label>
                <Input
                  value={selected.name}
                  onChange={(e) => {
                    const newName = e.target.value;
                    setGroups((prev) =>
                      prev.map((g) => (g.id === selected.id ? { ...g, name: newName } : g)),
                    );
                  }}
                  onBlur={() => updateGroup(selected.id, { name: selected.name })}
                  disabled={!canManageSelected && !isProtected}
                />
              </div>

              {/* Color */}
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-muted-foreground">{t("Color")}</label>
                <div className="flex flex-wrap gap-1.5">
                  {colors.map((c) => (
                    <button
                      key={c}
                      onClick={() => updateGroup(selected.id, { color: c })}
                      className={cn(
                        "h-7 w-7 rounded-lg transition-transform hover:scale-110",
                        selected.color === c && "ring-2 ring-foreground ring-offset-2 ring-offset-background",
                      )}
                      style={{ backgroundColor: c }}
                    >
                      {selected.color === c && (
                        <CheckIcon className="mx-auto h-3.5 w-3.5 text-white" />
                      )}
                    </button>
                  ))}
                </div>
              </div>

              {/* Hub permissions (not for protected groups) */}
              {!isProtected && (
                <div className="space-y-1.5">
                  <label className="text-xs font-medium text-muted-foreground">{t("Permissions")}</label>
                  <div className="flex flex-col gap-1">
                    {HUB_PERMISSION_LABELS.map((p) => {
                      const enabled = hasBit(selected.hub_permissions, p.bit);
                      const canToggle = canManageSelected && hasBit(perms?.hub_bits ?? 0, p.bit);
                      return (
                        <button
                          key={p.key}
                          onClick={() => canToggle && toggleBit(p.bit)}
                          disabled={!canToggle}
                          className={cn(
                            "flex items-center justify-between rounded-lg px-3 py-2 text-left text-sm transition-colors",
                            canToggle ? "hover:bg-muted" : "opacity-50",
                          )}
                        >
                          <span>{t(p.label)}</span>
                          <div
                            className={cn(
                              "flex h-5 w-9 items-center rounded-full transition-colors",
                              enabled ? "bg-primary" : "bg-muted",
                            )}
                          >
                            <span
                              className={cn(
                                "h-4 w-4 rounded-full bg-white shadow transition-transform",
                                enabled ? "translate-x-4.5" : "translate-x-0.5",
                              )}
                            />
                          </div>
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}

              {isProtected && (
                <p className="text-xs text-muted-foreground">
                  {t("This is a protected group. Only name and color can be changed.")}
                </p>
              )}

              {/* Delete */}
              {!isProtected && canManageSelected && (
                <>
                  <Separator />
                  <Button
                    variant="outline"
                    size="sm"
                    className="gap-1.5 border-red-200 text-red-500 hover:bg-red-50 dark:border-red-900 dark:text-red-400 dark:hover:bg-red-950"
                    onClick={() => deleteGroup(selected.id)}
                  >
                    <Trash2Icon className="h-3.5 w-3.5" />
                    {t("Delete role")}
                  </Button>
                </>
              )}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
