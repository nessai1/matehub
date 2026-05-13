import { FormEvent, useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { useAuth } from "@/lib/auth";
import { t } from "@/i18n";

const HUB_API = "/api/hub";
// Fallback used only until /v1/setup/status answers. The real hub_id ships
// as a string because Snowflake IDs blow past JS MAX_SAFE_INTEGER.
const FALLBACK_HUB_ID = "1";

export default function LoginPage() {
  const navigate = useNavigate();
  const { login } = useAuth();
  const [loginField, setLoginField] = useState("");
  const [password, setPassword] = useState("");
  const [rememberMe, setRememberMe] = useState(false);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [hubId, setHubId] = useState<string | null>(null);
  const [hubName, setHubName] = useState<string | null>(null);

  // Box deploys have exactly one hub and don't ship the hub_id baked into
  // the SPA bundle. /v1/setup/status is the unauthenticated source of
  // truth for "which hub does this deployment represent": it returns
  // `{needs_setup, hub_id?, hub_slug?, hub_name?}` and the hub identity
  // is populated once the first-run wizard has finished. Falling back to
  // FALLBACK_HUB_ID only covers dev where the seed pins hub_id=1.
  useEffect(() => {
    let cancelled = false;
    fetch(`${HUB_API}/v1/setup/status`)
      .then((r) => (r.ok ? r.json() : null))
      .then((data: { hub_id?: string; hub_name?: string } | null) => {
        if (cancelled || !data) return;
        if (data.hub_id) setHubId(data.hub_id);
        if (data.hub_name) setHubName(data.hub_name);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setLoading(true);

    try {
      const res = await fetch(`${HUB_API}/v1/auth/login`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          login: loginField,
          password,
          hub_id: hubId ?? FALLBACK_HUB_ID,
          remember_me: rememberMe,
        }),
      });

      if (!res.ok) {
        if (res.status === 401) setError("Invalid username or password");
        else if (res.status === 403) setError("You are not a member of this hub");
        else setError(`Login failed (${res.status})`);
        setLoading(false);
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
        refreshToken: data.refresh_token ?? undefined,
        expiresIn: data.expires_in,
        avatarUrl: data.avatar_url ?? undefined,
      });
      navigate("/hub");
    } catch {
      setError("Cannot reach server");
      setLoading(false);
    }
  };

  return (
    <div className="flex flex-col items-center">
      <div className="mb-8 text-center">
        <h1 className="text-2xl font-bold tracking-tight text-zinc-100">
          {t("Sign in")}
        </h1>
        <p className="mt-1 font-mono text-xs text-zinc-500">
          {hubName ?? " "}
        </p>
      </div>

      <div className="w-full rounded-lg border border-zinc-800 bg-zinc-900/80 p-8 backdrop-blur-sm">
        <form onSubmit={handleSubmit} className="flex flex-col gap-5">
          <div className="flex flex-col gap-2">
            <label className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase">
              {t("Username")}
            </label>
            <Input
              type="text"
              value={loginField}
              onChange={(e) => setLoginField(e.target.value)}
              placeholder="alice or alice@matehub.dev"
              required
              autoFocus
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-blue-500/30"
            />
          </div>

          <div className="flex flex-col gap-2">
            <label className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase">
              {t("Password")}
            </label>
            <Input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="--------"
              required
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-blue-500/30"
            />
          </div>

          <label className="flex items-center gap-2 cursor-pointer">
            <Checkbox
              checked={rememberMe}
              onCheckedChange={(checked) => setRememberMe(checked === true)}
              className="border-zinc-700 bg-zinc-950 data-[state=checked]:bg-blue-600 data-[state=checked]:border-blue-600"
            />
            <span className="font-mono text-xs text-zinc-400">{t("Remember me")}</span>
          </label>

          {error && (
            <p className="rounded border border-red-900/30 bg-red-950/20 px-3 py-2 font-mono text-xs text-red-400">
              {error}
            </p>
          )}

          <Button
            type="submit"
            disabled={loading}
            className="mt-1 w-full bg-blue-600 font-mono text-sm tracking-wide text-white hover:bg-blue-500 disabled:opacity-40 transition-colors"
            size="lg"
          >
            {loading ? (
              <span className="flex items-center gap-2">
                <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-white/30 border-t-white" />
                {t("Logging in...")}
              </span>
            ) : (
              t("Sign in")
            )}
          </Button>
        </form>
      </div>
    </div>
  );
}
