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
import { t } from "@/i18n";

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
        if (r.status === 404) throw new Error(t("Link does not exist"));
        if (r.status === 410) throw new Error(t("This link has already been used"));
        if (!r.ok) throw new Error(`${t("Error")}: ${r.status}`);
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
      setError(t("Enter your name"));
      return;
    }
    if (password.length < 6) {
      setError(t("Password too short (minimum 6 characters)"));
      return;
    }
    if (password !== passwordConfirm) {
      setError(t("Passwords don't match"));
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
        setError(t("This link has already been used"));
        setSubmitting(false);
        return;
      }
      if (res.status === 409) {
        setError(t("Login is taken"));
        setSubmitting(false);
        return;
      }
      if (!res.ok) {
        setError(`${t("Error")}: ${res.status}`);
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
      setError(t("Could not reach the server"));
      setSubmitting(false);
    }
  };

  if (loading) {
    return (
      <div className="flex flex-col items-center">
        <div className="w-full rounded-lg border border-zinc-800 bg-zinc-900/80 p-8 backdrop-blur-sm">
          <div className="flex flex-col items-center gap-4 animate-in fade-in duration-300">
            <div className="flex h-12 w-12 items-center justify-center rounded-full border border-zinc-700">
              <svg className="h-5 w-5 animate-spin text-zinc-500" viewBox="0 0 24 24" fill="none">
                <circle cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="2" className="opacity-20" />
                <path d="M12 2a10 10 0 0 1 10 10" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
              </svg>
            </div>
            <p className="font-mono text-sm text-zinc-400">{t("Checking link...")}</p>
          </div>
        </div>
      </div>
    );
  }

  if (loadError || !preview) {
    return (
      <div className="flex flex-col items-center">
        <div className="w-full rounded-lg border border-zinc-800 bg-zinc-900/80 p-8 backdrop-blur-sm">
          <div className="flex flex-col items-center gap-4 animate-in fade-in duration-300">
            <div className="flex h-12 w-12 items-center justify-center rounded-full border border-red-900/50 bg-red-950/30">
              <svg className="h-5 w-5 text-red-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <path d="M18 6 6 18M6 6l12 12" />
              </svg>
            </div>
            <div className="text-center">
              <p className="text-sm font-medium text-zinc-300">{loadError || t("Link is invalid")}</p>
              <p className="mt-1 font-mono text-[10px] text-zinc-600">
                {t("Ask the administrator to create a new one.")}
              </p>
            </div>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-col items-center">
      <div className="mb-8 text-center">
        <h1 className="text-2xl font-bold tracking-tight text-zinc-100">
          {t("Join %s", preview.hub_name)}
        </h1>
        <p className="mt-1 font-mono text-xs text-zinc-500">
          {t("Login")}: <span className="text-zinc-300">{preview.username}</span>
        </p>
      </div>

      <div className="w-full rounded-lg border border-zinc-800 bg-zinc-900/80 p-8 backdrop-blur-sm">
        <form onSubmit={submit} className="flex flex-col gap-5">
          <div className="flex items-center gap-4">
            <Avatar className="h-16 w-16 rounded-xl border border-zinc-700 bg-zinc-800">
              {avatarPreview ? (
                <AvatarImage src={avatarPreview} className="rounded-xl object-cover" />
              ) : (
                <AvatarFallback className="rounded-xl bg-zinc-800 text-2xl font-bold text-zinc-300">
                  {(displayName || preview.username).slice(0, 1).toUpperCase()}
                </AvatarFallback>
              )}
            </Avatar>
            <div>
              <Label
                htmlFor="invite-avatar"
                className="cursor-pointer font-mono text-[10px] tracking-widest text-zinc-500 uppercase hover:text-zinc-300 transition-colors"
              >
                {t("Upload avatar")}
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

          <div className="flex flex-col gap-2">
            <Label
              htmlFor="display-name"
              className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase"
            >
              {t("Your name")}
            </Label>
            <Input
              id="display-name"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              required
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-blue-500/30"
            />
          </div>

          <div className="flex flex-col gap-2">
            <Label
              htmlFor="invite-password"
              className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase"
            >
              {t("Password")}
            </Label>
            <Input
              id="invite-password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="--------"
              required
              autoComplete="new-password"
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-blue-500/30"
            />
          </div>

          <div className="flex flex-col gap-2">
            <Label
              htmlFor="invite-password-confirm"
              className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase"
            >
              {t("Confirm password")}
            </Label>
            <Input
              id="invite-password-confirm"
              type="password"
              value={passwordConfirm}
              onChange={(e) => setPasswordConfirm(e.target.value)}
              placeholder="--------"
              required
              autoComplete="new-password"
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-blue-500/30"
            />
          </div>

          {error && (
            <p className="rounded border border-red-900/30 bg-red-950/20 px-3 py-2 font-mono text-xs text-red-400">
              {error}
            </p>
          )}

          <Button
            type="submit"
            disabled={submitting}
            className="mt-1 w-full bg-blue-600 font-mono text-sm tracking-wide text-white hover:bg-blue-500 disabled:opacity-40 transition-colors"
            size="lg"
          >
            {submitting ? (
              <span className="flex items-center gap-2">
                <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-white/30 border-t-white" />
                {t("Creating...")}
              </span>
            ) : (
              t("Accept invitation")
            )}
          </Button>
        </form>
      </div>
    </div>
  );
}
