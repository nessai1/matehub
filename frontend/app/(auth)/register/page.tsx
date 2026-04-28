// Public landing for permanent invite links: /invite/{token}.
//
// On mount we hit the preview endpoint to validate the token and show
// the inviter's preset username. The visitor fills in display name,
// avatar and password; on submit the account is created and they're
// dropped straight into the hub. Single-use — refreshing this page
// after acceptance returns 410 Gone.

import { ChangeEvent, FormEvent, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useAuth } from "@/lib/auth";

const HUB_API = "/api/hub";

interface InvitationPreview {
  hub_name: string;
  username: string;
  email: string | null;
}

export default function InvitePage() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();
  const { login } = useAuth();

  const [preview, setPreview] = useState<InvitationPreview | null>(null);
  const [loadError, setLoadError] = useState("");
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");

  const [avatarFile, setAvatarFile] = useState<File | null>(null);
  const [avatarPreview, setAvatarPreview] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [password, setPassword] = useState("");
  const [passwordConfirm, setPasswordConfirm] = useState("");

  useEffect(() => {
    if (!token) return;
    fetch(`${HUB_API}/v1/invitations/${token}`)
      .then(async (r) => {
        if (r.status === 404) throw new Error("Ссылка не существует");
        if (r.status === 410) throw new Error("Эта ссылка уже использована");
        if (!r.ok) throw new Error(`Ошибка: ${r.status}`);
        return r.json();
      })
      .then((data: InvitationPreview) => setPreview(data))
      .catch((e: Error) => setLoadError(e.message))
      .finally(() => setLoading(false));
  }, [token]);

  const handleAvatarChange = (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0] ?? null;
    setAvatarFile(file);
    setAvatarPreview(file ? URL.createObjectURL(file) : "");
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!token || !preview) return;
    setError("");
    if (!displayName.trim()) {
      setError("Введите имя");
      return;
    }
    if (password.length < 6) {
      setError("Пароль слишком короткий (минимум 6 символов)");
      return;
    }
    if (password !== passwordConfirm) {
      setError("Пароли не совпадают");
      return;
    }
    setSubmitting(true);

    try {
      const res = await fetch(`${HUB_API}/v1/invitations/${token}/accept`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          display_name: displayName.trim(),
          password,
        }),
      });
      if (res.status === 410) {
        setError("Эта ссылка уже использована");
        setSubmitting(false);
        return;
      }
      if (res.status === 409) {
        setError("Логин занят");
        setSubmitting(false);
        return;
      }
      if (!res.ok) {
        setError(`Ошибка: ${res.status}`);
        setSubmitting(false);
        return;
      }
      const data = await res.json();

      if (avatarFile) {
        const fd = new FormData();
        fd.append("file", avatarFile);
        await fetch(`${HUB_API}/v1/hubs/${data.hub_id}/profile/avatar`, {
          method: "POST",
          headers: { Authorization: `Bearer ${data.access_token}` },
          body: fd,
        }).catch(() => {});
      }

      login({
        type: "permanent",
        userId: data.user_id,
        username: data.username,
        displayName: data.display_name,
        hubId: data.hub_id,
        hubSlug: data.hub_slug,
        token: data.access_token,
        refreshToken: data.refresh_token,
        expiresIn: data.expires_in,
      });
      navigate("/hub", { replace: true });
    } catch {
      setError("Не получилось связаться с сервером");
      setSubmitting(false);
    }
  };

  if (loading) {
    return (
      <div className="flex flex-col items-center gap-3 p-8">
        <div className="h-5 w-5 animate-spin rounded-full border-2 border-zinc-600 border-t-zinc-300" />
        <p className="font-mono text-xs text-zinc-500">Проверяем ссылку...</p>
      </div>
    );
  }

  if (loadError || !preview) {
    return (
      <div className="space-y-3 p-8 text-center">
        <h1 className="text-lg font-semibold">{loadError || "Ссылка недействительна"}</h1>
        <p className="text-sm text-muted-foreground">Попросите администратора создать новую.</p>
      </div>
    );
  }

  return (
    <form onSubmit={submit} className="space-y-4 p-2">
      <div className="space-y-1">
        <h1 className="text-xl font-semibold">Присоединиться к {preview.hub_name}</h1>
        <p className="text-sm text-muted-foreground">
          Логин: <span className="font-mono">{preview.username}</span>
        </p>
      </div>

      <div className="flex items-center gap-4">
        <Avatar className="h-16 w-16">
          {avatarPreview ? (
            <AvatarImage src={avatarPreview} />
          ) : (
            <AvatarFallback>{(displayName || preview.username).slice(0, 1).toUpperCase()}</AvatarFallback>
          )}
        </Avatar>
        <div>
          <Label htmlFor="invite-avatar" className="cursor-pointer text-sm text-muted-foreground hover:text-foreground">
            Загрузить аватар
          </Label>
          <Input
            id="invite-avatar"
            type="file"
            accept="image/*"
            className="hidden"
            onChange={handleAvatarChange}
          />
        </div>
      </div>

      <div className="space-y-2">
        <Label htmlFor="display-name">Ваше имя</Label>
        <Input
          id="display-name"
          value={displayName}
          onChange={(e) => setDisplayName(e.target.value)}
          required
        />
      </div>

      <div className="space-y-2">
        <Label htmlFor="invite-password">Пароль</Label>
        <Input
          id="invite-password"
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          required
          autoComplete="new-password"
        />
      </div>

      <div className="space-y-2">
        <Label htmlFor="invite-password-confirm">Повторите пароль</Label>
        <Input
          id="invite-password-confirm"
          type="password"
          value={passwordConfirm}
          onChange={(e) => setPasswordConfirm(e.target.value)}
          required
          autoComplete="new-password"
        />
      </div>

      {error && <p className="text-sm text-destructive">{error}</p>}

      <Button type="submit" className="w-full" disabled={submitting}>
        {submitting ? "Создаём..." : "Принять приглашение"}
      </Button>
    </form>
  );
}
