// "Add Teammates" — one popup, two tabs.
//   Temp link:       guest with TTL, comes through /join/{token}
//   Permanent user:  pre-allocated login, the invitee finishes signup
//                    at /invite/{token} (boxed; SaaS will mail later)
//
// On boxed deploys we don't send invite emails — admin copies the
// generated URL and shares it however they like.

import { useEffect, useMemo, useState } from "react";
import { format } from "date-fns";
import { Copy, Check, ChevronDownIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
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
import { t } from "@/i18n";

const HUB_API = "/api/hub";

const pad2 = (n: number) => n.toString().padStart(2, "0");

interface Group {
  id: string;
  name: string;
  is_default: boolean;
}

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function AddTeammatesDialog({ open, onOpenChange }: Props) {
  const { session } = useAuth();
  const [defaultGroupId, setDefaultGroupId] = useState<string | null>(null);

  // Load groups once we have a session — needed for the temp-user form.
  useEffect(() => {
    if (!session || !open) return;
    fetch(`${HUB_API}/v1/hubs/${session.hubId}/groups`, {
      headers: { Authorization: `Bearer ${session.token}` },
    })
      .then((r) => (r.ok ? r.json() : []))
      .then((groups: Group[]) => {
        const def = groups.find((g) => g.is_default);
        if (def) setDefaultGroupId(def.id);
      })
      .catch(() => {});
  }, [open, session]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{t("Invite participants")}</DialogTitle>
          <DialogDescription>
            {t("Temporary link expires by timer. Permanent link is bound to a login.")}
          </DialogDescription>
        </DialogHeader>

        <Tabs defaultValue="temp" className="mt-4">
          {/* Pill is content-sized (inline-flex w-fit, shadcn default) so the
              longer "Постоянный пользователь" gets natural breathing room
              instead of being squeezed into a forced 50% column. */}
          <TabsList>
            <TabsTrigger value="temp" className="px-4">
              {t("Temporary link")}
            </TabsTrigger>
            <TabsTrigger value="permanent" className="px-4">
              {t("Permanent user")}
            </TabsTrigger>
          </TabsList>

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

  const placeholderNick = useMemo(() => {
    if (!hubId) return "tempuser";
    const rand = Math.random().toString(36).slice(2, 8);
    return `tempuser-${hubId.slice(-4)}-${rand}`;
  }, [hubId]);

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
  const [error, setError] = useState("");

  const submit = async () => {
    if (!hubId || !token) return;
    if (!username.trim()) {
      setError("Введите логин");
      return;
    }
    setError("");
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
        setError(res.status === 403 ? "Нет прав на создание приглашений" : `Ошибка: ${res.status}`);
        setSubmitting(false);
        return;
      }
      const data = await res.json();
      setInviteUrl(`${window.location.origin}${data.invite_url}`);
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
        <Label htmlFor="invite-username">{t("Login")}</Label>
        <Input
          id="invite-username"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          autoComplete="off"
        />
      </div>

      <div className="space-y-2">
        <Label htmlFor="invite-email">{t("Email (optional)")}</Label>
        <Input
          id="invite-email"
          type="email"
          value={email}
          onChange={(e) => setEmail(e.target.value)}
        />
      </div>

      {error && <p className="text-sm text-destructive">{error}</p>}

      <Button onClick={submit} disabled={submitting} className="w-full">
        {submitting ? t("Creating...") : t("Create invitation")}
      </Button>
    </div>
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
