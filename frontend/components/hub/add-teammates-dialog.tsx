// "Add Teammates" — one popup, two tabs.
//   Temp link:       guest with TTL, comes through /join/{token}
//   Permanent user:  pre-allocated login, the invitee finishes signup
//                    at /invite/{token} (boxed; SaaS will mail later)
//
// On boxed deploys we don't send invite emails — admin copies the
// generated URL and shares it however they like.

import { useEffect, useMemo, useState } from "react";
import { Copy, Check } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Calendar } from "@/components/ui/calendar";
import { useAuth } from "@/lib/auth";

const HUB_API = "/api/hub";

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
          <DialogTitle>Пригласить участников</DialogTitle>
          <DialogDescription>
            Временная ссылка истекает по таймеру. Постоянная привязывается к логину.
          </DialogDescription>
        </DialogHeader>

        <Tabs defaultValue="temp">
          <TabsList className="grid w-full grid-cols-2">
            <TabsTrigger value="temp">Временная ссылка</TabsTrigger>
            <TabsTrigger value="permanent">Постоянный пользователь</TabsTrigger>
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
        <Label htmlFor="nickname">Имя гостя</Label>
        <Input
          id="nickname"
          value={nickname}
          onChange={(e) => setNickname(e.target.value)}
          placeholder={placeholderNick}
        />
      </div>

      <div className="space-y-2">
        <Label>Действует до</Label>
        <Calendar
          mode="single"
          selected={expiresAt}
          onSelect={(d) => d && setExpiresAt(d)}
          disabled={(d) => d <= new Date()}
          className="rounded-md border"
        />
      </div>

      {error && <p className="text-sm text-destructive">{error}</p>}

      <Button onClick={submit} disabled={submitting} className="w-full">
        {submitting ? "Создаём..." : "Сгенерировать ссылку"}
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
        <Label htmlFor="invite-username">Логин</Label>
        <Input
          id="invite-username"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          autoComplete="off"
        />
      </div>

      <div className="space-y-2">
        <Label htmlFor="invite-email">Email (необязательно)</Label>
        <Input
          id="invite-email"
          type="email"
          value={email}
          onChange={(e) => setEmail(e.target.value)}
        />
      </div>

      {error && <p className="text-sm text-destructive">{error}</p>}

      <Button onClick={submit} disabled={submitting} className="w-full">
        {submitting ? "Создаём..." : "Создать приглашение"}
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
      <Label>Готово — отправьте ссылку приглашённому:</Label>
      <div className="flex gap-2">
        <Input value={url} readOnly onFocus={(e) => e.currentTarget.select()} />
        <Button variant="outline" size="icon" onClick={copy} type="button">
          {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
        </Button>
      </div>
      <Button variant="ghost" onClick={onReset} className="w-full">
        Создать ещё одну
      </Button>
    </div>
  );
}
