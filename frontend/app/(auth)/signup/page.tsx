// Public landing for general invite-link signups: /signup/{token}.
//
// On mount we hit the preview endpoint to validate the token and surface
// "Joining {hub_name}". The visitor picks their own login + display name
// + email + password; on submit the account is created and they're
// dropped straight into the hub.
//
// Multi-use: the same link works until expires_at OR uses_count hits
// max_uses. The 6th visitor on a max_uses=5 link gets 410.

import { FormEvent, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useAuth } from "@/lib/auth";
import { t } from "@/i18n";

const HUB_API = "/api/hub";

interface InviteLinkPreview {
  hub_name: string;
  hub_slug: string;
  expires_at: string;
  max_uses: number | null;
  uses_count: number;
  group_name: string | null;
}

export default function SignupPage() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();
  const { login } = useAuth();

  const [preview, setPreview] = useState<InviteLinkPreview | null>(null);
  const [loadError, setLoadError] = useState("");
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);

  const [username, setUsername] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [passwordConfirm, setPasswordConfirm] = useState("");

  // Field-scoped errors to mirror discriminated server payloads.
  const [usernameErr, setUsernameErr] = useState("");
  const [emailErr, setEmailErr] = useState("");
  const [passwordErr, setPasswordErr] = useState("");
  const [displayNameErr, setDisplayNameErr] = useState("");
  const [formErr, setFormErr] = useState("");

  useEffect(() => {
    if (!token) return;
    fetch(`${HUB_API}/v1/invite-links/${token}`)
      .then(async (r) => {
        if (r.status === 404) throw new Error(t("Link does not exist"));
        if (r.status === 410) throw new Error(t("This link is no longer valid"));
        if (!r.ok) throw new Error(`${t("Error")}: ${r.status}`);
        return r.json();
      })
      .then((data: InviteLinkPreview) => setPreview(data))
      .catch((e: Error) => setLoadError(e.message))
      .finally(() => setLoading(false));
  }, [token]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!token || !preview) return;

    setUsernameErr("");
    setEmailErr("");
    setPasswordErr("");
    setDisplayNameErr("");
    setFormErr("");

    if (password !== passwordConfirm) {
      setPasswordErr(t("Passwords don't match"));
      return;
    }

    setSubmitting(true);
    try {
      const res = await fetch(`${HUB_API}/v1/invite-links/${token}/redeem`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          username: username.trim(),
          password,
          display_name: displayName.trim(),
          email: email.trim(),
        }),
      });

      if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        const code = (body as { error?: string }).error;
        switch (code) {
          case "username_invalid":
            setUsernameErr(
              t("Login must be 3–32 chars: letters, digits, . _ -"),
            );
            break;
          case "username_taken":
            setUsernameErr(t("Login is already taken"));
            break;
          case "email_invalid":
            setEmailErr(t("Invalid email format"));
            break;
          case "email_taken":
            setEmailErr(t("Email is already in use"));
            break;
          case "password_too_short":
            setPasswordErr(t("Password too short (minimum 6 characters)"));
            break;
          case "display_name_invalid":
            setDisplayNameErr(t("Enter your name"));
            break;
          case "link_unavailable":
            setFormErr(t("This link is no longer valid"));
            break;
          default:
            setFormErr(`${t("Error")}: ${res.status}`);
        }
        setSubmitting(false);
        return;
      }

      const data = await res.json();
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
      setFormErr(t("Could not reach the server"));
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
              <p className="text-sm font-medium text-zinc-300">
                {loadError || t("Link is invalid")}
              </p>
              <p className="mt-1 font-mono text-[10px] text-zinc-600">
                {t("Ask the administrator to create a new one.")}
              </p>
            </div>
          </div>
        </div>
      </div>
    );
  }

  const slotsLeft =
    preview.max_uses != null ? preview.max_uses - preview.uses_count : null;

  return (
    <div className="flex flex-col items-center">
      <div className="mb-8 text-center">
        <h1 className="text-2xl font-bold tracking-tight text-zinc-100">
          {t("Join %s", preview.hub_name)}
        </h1>
        {preview.group_name && (
          <p className="mt-1 font-mono text-xs text-zinc-500">
            {t("Group")}: <span className="text-zinc-300">{preview.group_name}</span>
          </p>
        )}
        {slotsLeft != null && (
          <p className="mt-1 font-mono text-[10px] text-zinc-600">
            {t("%d of %d spots left", slotsLeft, preview.max_uses ?? 0)}
          </p>
        )}
      </div>

      <div className="w-full rounded-lg border border-zinc-800 bg-zinc-900/80 p-8 backdrop-blur-sm">
        <form onSubmit={submit} className="flex flex-col gap-5">
          <div className="flex flex-col gap-2">
            <Label
              htmlFor="signup-username"
              className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase"
            >
              {t("Login")}
            </Label>
            <Input
              id="signup-username"
              value={username}
              onChange={(e) => {
                setUsername(e.target.value);
                if (usernameErr) setUsernameErr("");
              }}
              autoComplete="username"
              required
              aria-invalid={!!usernameErr}
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-blue-500/30"
            />
            {usernameErr && (
              <p className="font-mono text-xs text-red-400">{usernameErr}</p>
            )}
          </div>

          <div className="flex flex-col gap-2">
            <Label
              htmlFor="signup-display-name"
              className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase"
            >
              {t("Your name")}
            </Label>
            <Input
              id="signup-display-name"
              value={displayName}
              onChange={(e) => {
                setDisplayName(e.target.value);
                if (displayNameErr) setDisplayNameErr("");
              }}
              required
              aria-invalid={!!displayNameErr}
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-blue-500/30"
            />
            {displayNameErr && (
              <p className="font-mono text-xs text-red-400">{displayNameErr}</p>
            )}
          </div>

          <div className="flex flex-col gap-2">
            <Label
              htmlFor="signup-email"
              className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase"
            >
              {t("Email")}
            </Label>
            <Input
              id="signup-email"
              type="email"
              value={email}
              onChange={(e) => {
                setEmail(e.target.value);
                if (emailErr) setEmailErr("");
              }}
              autoComplete="email"
              required
              aria-invalid={!!emailErr}
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-blue-500/30"
            />
            {emailErr && (
              <p className="font-mono text-xs text-red-400">{emailErr}</p>
            )}
          </div>

          <div className="flex flex-col gap-2">
            <Label
              htmlFor="signup-password"
              className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase"
            >
              {t("Password")}
            </Label>
            <Input
              id="signup-password"
              type="password"
              value={password}
              onChange={(e) => {
                setPassword(e.target.value);
                if (passwordErr) setPasswordErr("");
              }}
              placeholder="--------"
              required
              autoComplete="new-password"
              aria-invalid={!!passwordErr}
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-blue-500/30"
            />
            {passwordErr && (
              <p className="font-mono text-xs text-red-400">{passwordErr}</p>
            )}
          </div>

          <div className="flex flex-col gap-2">
            <Label
              htmlFor="signup-password-confirm"
              className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase"
            >
              {t("Confirm password")}
            </Label>
            <Input
              id="signup-password-confirm"
              type="password"
              value={passwordConfirm}
              onChange={(e) => setPasswordConfirm(e.target.value)}
              placeholder="--------"
              required
              autoComplete="new-password"
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-blue-500/30"
            />
          </div>

          {formErr && (
            <p className="rounded border border-red-900/30 bg-red-950/20 px-3 py-2 font-mono text-xs text-red-400">
              {formErr}
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
              t("Sign up")
            )}
          </Button>
        </form>
      </div>
    </div>
  );
}
