// "Add Teammates" — one popup, three tabs.
//   Invite link:     shared link, multi-use, deadline + optional cap.
//                    Visitor self-registers at /signup/{token}. Default tab.
//   Temp link:       guest with TTL, comes through /join/{token}.
//   Permanent user:  pre-allocated login, the invitee finishes signup
//                    at /invite/{token} (boxed; SaaS will mail later).
//
// On boxed deploys we don't send invite emails — admin copies the
// generated URL and shares it however they like.

import { useEffect, useMemo, useState } from "react";
import { format } from "date-fns";
import { Copy, Check, CheckIcon, ChevronDownIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Calendar } from "@/components/ui/calendar";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { useAuth } from "@/lib/auth";
import { invalidatePendingInvites } from "@/hooks/use-pending-invites";
import { usePermissions } from "@/hooks/use-permissions";
import { t } from "@/i18n";

const HUB_API = "/api/hub";

const pad2 = (n: number) => n.toString().padStart(2, "0");

interface Group {
  id: string;
  name: string;
  color: string | null;
  position: number;
  is_default: boolean;
}

/// Filter the picker to groups the caller is allowed to assign:
///   - Anything strictly *below* the caller's top group (lower priority,
///     i.e. greater `position`). Equal-position would let an admin invite
///     someone to their own tier; we forbid that.
///   - Never the `admin` group itself, regardless of position. That tier
///     is reserved for explicit promotion via the member-card editor,
///     which has the creator-only check.
function assignableGroups(groups: Group[], myTopPosition: number): Group[] {
  return groups
    .filter((g) => g.position > myTopPosition)
    .filter((g) => g.name.toLowerCase() !== "admin");
}

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function AddTeammatesDialog({ open, onOpenChange }: Props) {
  const { session } = useAuth();
  const { perms } = usePermissions();
  const [groups, setGroups] = useState<Group[]>([]);

  // Caller can only invite into groups beneath their top tier (and never
  // into `admin`). top_position defaults to a sentinel so an unloaded
  // perms response still shows the regular default group.
  const myTopPosition = perms?.top_position ?? Number.MAX_SAFE_INTEGER;
  const pickable = useMemo(
    () => assignableGroups(groups, myTopPosition),
    [groups, myTopPosition],
  );
  const defaultGroupId = useMemo(
    // Pick the in-hub default if it's still assignable; otherwise the
    // highest-position (most-junior) group the caller can target.
    () =>
      pickable.find((g) => g.is_default)?.id ??
      pickable[pickable.length - 1]?.id ??
      null,
    [pickable],
  );

  // Load groups once we have a session — needed for both the temp-user form
  // (default group only) and the invite-link form (full picker).
  useEffect(() => {
    if (!session || !open) return;
    fetch(`${HUB_API}/v1/hubs/${session.hubId}/groups`, {
      headers: { Authorization: `Bearer ${session.token}` },
    })
      .then((r) => (r.ok ? r.json() : []))
      .then((data: Group[]) => setGroups(data))
      .catch(() => {});
  }, [open, session]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* `sm:max-w-2xl` (42rem). The DialogContent default (`dialog.tsx`)
          declares `sm:max-w-md`, so a bare `max-w-xl` here doesn't win at
          ≥sm breakpoints — the media-query'd default keeps the panel at
          28rem. The `sm:`-prefixed override does. The previous panel
          width was too narrow for three full-name TabsTriggers
          ("Ссылка-приглашение" + "Временная ссылка" +
          "Постоянный пользователь") plus the horizontal date+time row,
          pushing the time input and group select past the right edge. */}
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("Invite participants")}</DialogTitle>
          <DialogDescription>
            {t("Share a link, send a temporary guest pass, or pre-allocate a login.")}
          </DialogDescription>
        </DialogHeader>

        <Tabs defaultValue="link" className="mt-4">
          {/* Pill is content-sized (inline-flex w-fit, shadcn default) so the
              longer labels get natural breathing room instead of being
              squeezed into forced equal columns. */}
          <TabsList>
            <TabsTrigger value="link" className="px-4">
              {t("Invite link")}
            </TabsTrigger>
            <TabsTrigger value="temp" className="px-4">
              {t("Temporary link")}
            </TabsTrigger>
            <TabsTrigger value="permanent" className="px-4">
              {t("Permanent user")}
            </TabsTrigger>
          </TabsList>

          <TabsContent value="link" className="mt-4">
            <InviteLinkForm
              hubId={session?.hubId}
              token={session?.token}
              groups={pickable}
              defaultGroupId={defaultGroupId}
            />
          </TabsContent>

          <TabsContent value="temp" className="mt-4">
            <TempInviteForm
              hubId={session?.hubId}
              token={session?.token}
              groupId={defaultGroupId}
            />
          </TabsContent>

          <TabsContent value="permanent" className="mt-4">
            <PermanentInviteForm
              hubId={session?.hubId}
              token={session?.token}
            />
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}

// ── Invite link tab ─────────────────────────────────────────────

function InviteLinkForm({
  hubId,
  token,
  groups,
  defaultGroupId,
}: {
  hubId?: string;
  token?: string;
  groups: Group[];
  defaultGroupId: string | null;
}) {
  const defaultExpiry = useMemo(() => {
    const d = new Date();
    d.setDate(d.getDate() + 7);
    return d;
  }, []);
  const [expiresAt, setExpiresAt] = useState<Date>(defaultExpiry);
  const [dateOpen, setDateOpen] = useState(false);
  const [limited, setLimited] = useState(false);
  const [maxUses, setMaxUses] = useState("10");
  // groupId is null until the user explicitly picks something; the Select
  // shows the hub default as the placeholder-fallback. Keeping state strictly
  // user-driven avoids the "syncing prop into state in useEffect" cascade
  // that react-compiler (rightly) flags.
  const [groupId, setGroupId] = useState<string | null>(null);
  const effectiveGroupId = groupId ?? defaultGroupId;
  const [inviteUrl, setInviteUrl] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [maxUsesErr, setMaxUsesErr] = useState("");
  const [formErr, setFormErr] = useState("");

  const submit = async () => {
    if (!hubId || !token) return;
    setMaxUsesErr("");
    setFormErr("");

    let cap: number | null = null;
    if (limited) {
      const n = Number(maxUses);
      if (!Number.isFinite(n) || !Number.isInteger(n) || n < 1) {
        setMaxUsesErr(t("Enter a positive whole number"));
        return;
      }
      cap = n;
    }

    setSubmitting(true);
    try {
      const res = await fetch(`${HUB_API}/v1/hubs/${hubId}/invite-links`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify({
          expires_at: expiresAt.toISOString(),
          max_uses: cap,
          group_id: effectiveGroupId,
        }),
      });
      if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        const code = (body as { error?: string }).error;
        switch (code) {
          case "expires_in_past":
            setFormErr(t("Pick a future date"));
            break;
          case "invalid_max_uses":
            setMaxUsesErr(t("Enter a positive whole number"));
            break;
          case "invalid_group":
            setFormErr(t("Selected group is no longer available"));
            break;
          case "forbidden":
            setFormErr(t("No permission to create invitations"));
            break;
          default:
            setFormErr(`${t("Error")}: ${res.status}`);
        }
        setSubmitting(false);
        return;
      }
      const data = await res.json();
      setInviteUrl(`${window.location.origin}${data.invite_url}`);
      invalidatePendingInvites(hubId);
    } catch {
      setFormErr(t("Could not reach the server"));
    }
    setSubmitting(false);
  };

  if (inviteUrl) {
    return <InviteResult url={inviteUrl} onReset={() => setInviteUrl("")} />;
  }

  return (
    // `flex flex-col gap-5` instead of `space-y-5`: the latter relies on
    // adjacent-sibling margins, which the shadcn `<FieldGroup>`'s own
    // `flex-col gap-7` was eating, leaving the date row visually flush
    // against the checkbox below. Explicit gap on a flex parent works
    // unconditionally.
    <div className="flex flex-col gap-5">
      <FieldGroup className="flex-row">
        <Field>
          <FieldLabel htmlFor="link-expires-date">{t("Valid until")}</FieldLabel>
          <Popover open={dateOpen} onOpenChange={setDateOpen}>
            <PopoverTrigger asChild>
              <Button
                id="link-expires-date"
                variant="outline"
                className="w-40 justify-between font-normal"
              >
                {format(expiresAt, "PPP")}
                <ChevronDownIcon className="h-4 w-4 opacity-50" />
              </Button>
            </PopoverTrigger>
            <PopoverContent className="w-auto overflow-hidden p-0" align="start">
              <Calendar
                mode="single"
                selected={expiresAt}
                captionLayout="dropdown"
                defaultMonth={expiresAt}
                onSelect={(d) => {
                  if (!d) return;
                  const next = new Date(d);
                  next.setHours(
                    expiresAt.getHours(),
                    expiresAt.getMinutes(),
                    0,
                    0,
                  );
                  setExpiresAt(next);
                  setDateOpen(false);
                }}
                disabled={(d) => {
                  const startOfToday = new Date();
                  startOfToday.setHours(0, 0, 0, 0);
                  return d < startOfToday;
                }}
              />
            </PopoverContent>
          </Popover>
        </Field>
        <Field className="w-32">
          <FieldLabel htmlFor="link-expires-time">{t("Time")}</FieldLabel>
          <Input
            id="link-expires-time"
            type="time"
            step="60"
            value={`${pad2(expiresAt.getHours())}:${pad2(expiresAt.getMinutes())}`}
            onChange={(e) => {
              const [h, m] = e.target.value.split(":").map(Number);
              if (Number.isNaN(h) || Number.isNaN(m)) return;
              const next = new Date(expiresAt);
              next.setHours(h, m, 0, 0);
              setExpiresAt(next);
            }}
            className="appearance-none [&::-webkit-calendar-picker-indicator]:hidden [&::-webkit-calendar-picker-indicator]:appearance-none"
          />
        </Field>
      </FieldGroup>

      {/* Checkbox + conditional max-uses live in one block so they read as a
          single "limit" choice. The outer space-y-5 keeps it visually
          separated from the group picker beneath. */}
      <div className="space-y-3">
        <div className="flex items-center gap-2.5">
          <Checkbox
            id="link-limited"
            checked={limited}
            onCheckedChange={(v) => {
              setLimited(v === true);
              if (v !== true) setMaxUsesErr("");
            }}
          />
          <Label htmlFor="link-limited" className="cursor-pointer font-normal">
            {t("Limit number of registrations")}
          </Label>
        </div>

        {limited && (
          <div className="space-y-2 pl-6">
            <Label htmlFor="link-max-uses">{t("Max registrations")}</Label>
            <Input
              id="link-max-uses"
              type="number"
              min={1}
              value={maxUses}
              onChange={(e) => {
                setMaxUses(e.target.value);
                if (maxUsesErr) setMaxUsesErr("");
              }}
              aria-invalid={!!maxUsesErr}
            />
            {maxUsesErr && (
              <p className="text-xs text-destructive">{maxUsesErr}</p>
            )}
          </div>
        )}
      </div>

      <div className="space-y-2">
        <Label>{t("Add to group")}</Label>
        <GroupPicker
          groups={groups}
          selectedId={effectiveGroupId}
          onSelect={setGroupId}
        />
      </div>

      {formErr && <p className="text-sm text-destructive">{formErr}</p>}

      <Button onClick={submit} disabled={submitting} className="w-full">
        {submitting ? t("Creating...") : t("Generate link")}
      </Button>
    </div>
  );
}

// ── Temp tab ────────────────────────────────────────────────────

function TempInviteForm({
  hubId,
  token,
  groupId,
}: {
  hubId?: string;
  token?: string;
  groupId: string | null;
}) {
  const defaultExpiry = useMemo(() => {
    const d = new Date();
    d.setDate(d.getDate() + 7);
    return d;
  }, []);
  const [nickname, setNickname] = useState("");
  const [expiresAt, setExpiresAt] = useState<Date>(defaultExpiry);
  const [dateOpen, setDateOpen] = useState(false);
  const [inviteUrl, setInviteUrl] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");

  // Random suffix is generated once per dialog mount; recomputing it on every
  // render would be impure and would also flicker the placeholder text.
  const [randSuffix] = useState(() => Math.random().toString(36).slice(2, 8));
  const placeholderNick = useMemo(
    () => (hubId ? `tempuser-${hubId.slice(-4)}-${randSuffix}` : "tempuser"),
    [hubId, randSuffix],
  );

  const submit = async () => {
    if (!hubId || !token || !groupId) {
      setError("Не загружены группы — попробуйте ещё раз через секунду");
      return;
    }
    setError("");
    setSubmitting(true);

    const ttlSeconds = Math.max(
      60,
      Math.floor((expiresAt.getTime() - Date.now()) / 1000),
    );

    try {
      const res = await fetch(`${HUB_API}/v1/hubs/${hubId}/temp-users`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify({
          nickname: nickname.trim() || placeholderNick,
          group_id: groupId,
          ttl_seconds: ttlSeconds,
        }),
      });
      if (!res.ok) {
        setError(`Ошибка: ${res.status}`);
        setSubmitting(false);
        return;
      }
      const data = await res.json();
      // Backend returns relative `/join/{token}` — make it absolute.
      setInviteUrl(`${window.location.origin}${data.invite_url}`);
      invalidatePendingInvites(hubId);
    } catch {
      setError("Не получилось связаться с сервером");
    }
    setSubmitting(false);
  };

  if (inviteUrl) {
    return <InviteResult url={inviteUrl} onReset={() => setInviteUrl("")} />;
  }

  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <Label htmlFor="nickname">{t("Guest name")}</Label>
        <Input
          id="nickname"
          value={nickname}
          onChange={(e) => setNickname(e.target.value)}
          placeholder={placeholderNick}
        />
      </div>

      <FieldGroup className="flex-row">
        <Field>
          <FieldLabel htmlFor="expires-date">{t("Valid until")}</FieldLabel>
          <Popover open={dateOpen} onOpenChange={setDateOpen}>
            <PopoverTrigger asChild>
              <Button
                id="expires-date"
                variant="outline"
                className="w-40 justify-between font-normal"
              >
                {format(expiresAt, "PPP")}
                <ChevronDownIcon className="h-4 w-4 opacity-50" />
              </Button>
            </PopoverTrigger>
            <PopoverContent className="w-auto overflow-hidden p-0" align="start">
              <Calendar
                mode="single"
                selected={expiresAt}
                captionLayout="dropdown"
                defaultMonth={expiresAt}
                onSelect={(d) => {
                  if (!d) return;
                  // Calendar lands on 00:00 — keep the time the user picked
                  // in the sibling input.
                  const next = new Date(d);
                  next.setHours(
                    expiresAt.getHours(),
                    expiresAt.getMinutes(),
                    0,
                    0,
                  );
                  setExpiresAt(next);
                  setDateOpen(false);
                }}
                disabled={(d) => {
                  // Compare at day granularity: today is OK, only past dates
                  // are blocked. (`new Date()` is the current moment, so a
                  // raw `d <= new Date()` would refuse today after 00:00.)
                  const startOfToday = new Date();
                  startOfToday.setHours(0, 0, 0, 0);
                  return d < startOfToday;
                }}
              />
            </PopoverContent>
          </Popover>
        </Field>
        <Field className="w-32">
          <FieldLabel htmlFor="expires-time">{t("Time")}</FieldLabel>
          <Input
            id="expires-time"
            type="time"
            step="60"
            value={`${pad2(expiresAt.getHours())}:${pad2(expiresAt.getMinutes())}`}
            onChange={(e) => {
              const [h, m] = e.target.value.split(":").map(Number);
              if (Number.isNaN(h) || Number.isNaN(m)) return;
              const next = new Date(expiresAt);
              next.setHours(h, m, 0, 0);
              setExpiresAt(next);
            }}
            className="appearance-none [&::-webkit-calendar-picker-indicator]:hidden [&::-webkit-calendar-picker-indicator]:appearance-none"
          />
        </Field>
      </FieldGroup>

      {error && <p className="text-sm text-destructive">{error}</p>}

      <Button onClick={submit} disabled={submitting} className="w-full">
        {submitting ? t("Creating...") : t("Generate link")}
      </Button>
    </div>
  );
}

// ── Permanent tab ───────────────────────────────────────────────

function PermanentInviteForm({
  hubId,
  token,
}: {
  hubId?: string;
  token?: string;
}) {
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [inviteUrl, setInviteUrl] = useState("");
  const [submitting, setSubmitting] = useState(false);
  // Field-scoped errors so the user sees which input is wrong, plus a
  // top-level "form" slot for everything that isn't tied to a field
  // (forbidden, network, unknown status).
  const [usernameErr, setUsernameErr] = useState("");
  const [emailErr, setEmailErr] = useState("");
  const [formErr, setFormErr] = useState("");

  // Maps the backend's discriminated `{error: "..."}` payload to either a
  // field-scoped message or a top-level one. Keeps the translation table in
  // one place rather than scattered through if/elses.
  const applyServerError = (code: string | undefined, fallbackStatus: number) => {
    switch (code) {
      case "username_required":
        setUsernameErr(t("Enter login"));
        return;
      case "username_taken":
        setUsernameErr(t("Login is already taken"));
        return;
      case "username_pending":
        setUsernameErr(t("There's already a pending invite for this login"));
        return;
      case "invalid_email":
        setEmailErr(t("Invalid email format"));
        return;
      case "email_taken":
        setEmailErr(t("Email is already in use"));
        return;
      case "email_pending":
        setEmailErr(t("There's already a pending invite for this email"));
        return;
      case "forbidden":
        setFormErr(t("No permission to create invitations"));
        return;
      default:
        setFormErr(`${t("Error")}: ${fallbackStatus}`);
    }
  };

  const submit = async () => {
    if (!hubId || !token) return;
    setUsernameErr("");
    setEmailErr("");
    setFormErr("");

    if (!username.trim()) {
      setUsernameErr(t("Enter login"));
      return;
    }

    setSubmitting(true);
    try {
      const res = await fetch(`${HUB_API}/v1/hubs/${hubId}/invitations`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify({
          username: username.trim(),
          email: email.trim() || null,
        }),
      });
      if (!res.ok) {
        // Body is `{error: "..."}` for all our 4xx; a 5xx might be empty
        // text, hence the .catch fallback.
        const body = await res.json().catch(() => ({}));
        applyServerError(
          (body as { error?: string }).error,
          res.status,
        );
        setSubmitting(false);
        return;
      }
      const data = await res.json();
      setInviteUrl(`${window.location.origin}${data.invite_url}`);
      invalidatePendingInvites(hubId);
    } catch {
      setFormErr(t("Could not reach the server"));
    }
    setSubmitting(false);
  };

  if (inviteUrl) {
    return <InviteResult url={inviteUrl} onReset={() => setInviteUrl("")} />;
  }

  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <Label htmlFor="invite-username">{t("Login")}</Label>
        <Input
          id="invite-username"
          value={username}
          onChange={(e) => {
            setUsername(e.target.value);
            if (usernameErr) setUsernameErr("");
          }}
          autoComplete="off"
          aria-invalid={!!usernameErr}
        />
        {usernameErr && <p className="text-xs text-destructive">{usernameErr}</p>}
      </div>

      <div className="space-y-2">
        <Label htmlFor="invite-email">{t("Email (optional)")}</Label>
        <Input
          id="invite-email"
          type="email"
          value={email}
          onChange={(e) => {
            setEmail(e.target.value);
            if (emailErr) setEmailErr("");
          }}
          aria-invalid={!!emailErr}
        />
        {emailErr && <p className="text-xs text-destructive">{emailErr}</p>}
      </div>

      {formErr && <p className="text-sm text-destructive">{formErr}</p>}

      <Button onClick={submit} disabled={submitting} className="w-full">
        {submitting ? t("Creating...") : t("Create invitation")}
      </Button>
    </div>
  );
}

// ── Group picker (mirrors member-card.tsx breadcrumb style) ─────
//
// Single-select cousin of the multi-select group editor in member-card.tsx.
// Shows the chosen group as a colored-dot + name pill, opens a Command-
// based picker on click. Same visual language so admins recognize it.

function GroupPicker({
  groups,
  selectedId,
  onSelect,
}: {
  groups: Group[];
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const selected = groups.find((g) => g.id === selectedId) ?? null;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          role="combobox"
          aria-expanded={open}
          className="w-full justify-between font-normal"
        >
          {selected ? (
            <span className="flex items-center gap-2">
              <span
                className="h-2 w-2 shrink-0 rounded-full"
                style={{ backgroundColor: selected.color || "#666" }}
              />
              <span className="truncate">{selected.name}</span>
              {selected.is_default && (
                <span className="text-xs text-muted-foreground">
                  · {t("default")}
                </span>
              )}
            </span>
          ) : (
            <span className="text-muted-foreground">{t("Select group")}</span>
          )}
          <ChevronDownIcon className="h-4 w-4 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[--radix-popover-trigger-width] p-0"
        align="start"
      >
        <Command>
          <CommandInput placeholder={t("Search groups...")} />
          <CommandList>
            <CommandEmpty>{t("No groups found")}</CommandEmpty>
            <CommandGroup>
              {groups.map((g) => {
                const isSelected = g.id === selectedId;
                return (
                  <CommandItem
                    key={g.id}
                    onSelect={() => {
                      onSelect(g.id);
                      setOpen(false);
                    }}
                  >
                    <span
                      className="mr-2 h-2 w-2 rounded-full"
                      style={{ backgroundColor: g.color || "#666" }}
                    />
                    <span>{g.name}</span>
                    {g.is_default && (
                      <span className="ml-1 text-xs text-muted-foreground">
                        · {t("default")}
                      </span>
                    )}
                    {isSelected && (
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
  );
}

// ── Shared result ───────────────────────────────────────────────

function InviteResult({ url, onReset }: { url: string; onReset: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Fallback: select for manual copy
    }
  };
  return (
    <div className="space-y-3">
      <Label>{t("Done — share the link with the invitee:")}</Label>
      <div className="flex gap-2">
        <Input value={url} readOnly onFocus={(e) => e.currentTarget.select()} />
        <Button variant="outline" size="icon" onClick={copy} type="button">
          {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
        </Button>
      </div>
      <Button variant="ghost" onClick={onReset} className="w-full">
        {t("Create another")}
      </Button>
    </div>
  );
}
