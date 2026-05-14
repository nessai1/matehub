import { FormEvent, useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { useAuth } from "@/lib/auth";
import { t } from "@/i18n";

const HUB_API = "/api/hub";
// Fallback used only as a last-ditch path when /v1/setup/status is
// completely unreachable AND the user still insists on submitting.
// The real hub_id ships as a string because Snowflake IDs blow past
// JS MAX_SAFE_INTEGER.
const FALLBACK_HUB_ID = "1";

type StatusResponse = {
  needs_setup?: boolean;
  hub_id?: string;
  hub_slug?: string;
  hub_name?: string;
};

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
  // statusLoaded flips once the GET /v1/setup/status request has resolved
  // ONE WAY OR THE OTHER (success, 5xx, or network error). Used to gate
  // the submit button — see the race-comment on handleSubmit. We keep
  // the response error in `statusError` so we can surface a banner;
  // we don't block the form on the error, because dev seeds with hub_id=1
  // are a perfectly valid fallback path.
  const [statusLoaded, setStatusLoaded] = useState(false);
  const [statusError, setStatusError] = useState(false);

  // Box deploys have exactly one hub and don't ship the hub_id baked into
  // the SPA bundle. /v1/setup/status is the unauthenticated source of
  // truth for "which hub does this deployment represent": it returns
  // `{needs_setup, hub_id?, hub_slug?, hub_name?}` and the hub identity
  // is populated once the first-run wizard has finished.
  //
  // Returns the freshly-fetched hub_id (or null) so handleSubmit can
  // await it without waiting for the next React render cycle to flip
  // the state setters.
  const fetchStatus = useCallback(async (): Promise<string | null> => {
    try {
      const r = await fetch(`${HUB_API}/v1/setup/status`);
      if (!r.ok) {
        // 5xx is the case the silent-catch was hiding: bind a banner so
        // the box operator who is also the only user knows the backend
        // is dead instead of staring at a 403 minute later.
        console.error("setup/status failed", r.status);
        setStatusError(true);
        return null;
      }
      const data = (await r.json()) as StatusResponse;
      if (data.hub_id) setHubId(data.hub_id);
      if (data.hub_name) setHubName(data.hub_name);
      setStatusError(false);
      return data.hub_id ?? null;
    } catch (e) {
      // Network/CORS/DNS failures land here. Same banner path as 5xx —
      // distinction matters for the dev console (we logged it), not for
      // the user, who in both cases has a backend they can't reach.
      console.error("setup/status network error", e);
      setStatusError(true);
      return null;
    } finally {
      setStatusLoaded(true);
    }
  }, []);

  useEffect(() => {
    // Initial-data fetch on mount. The lint rule below would normally
    // flag a setState-inside-effect chain (because fetchStatus ends up
    // calling setHubId / setStatusLoaded / setStatusError), but that's
    // exactly what "load remote data when the page opens" requires —
    // see React docs "You Might Not Need an Effect / Fetching data".
    // No alternative (server-component / loader) is available here:
    // the page is a client-only React Router route, and the data is
    // also consumed during submit, not just initial render.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void fetchStatus();
  }, [fetchStatus]);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setLoading(true);

    // Race fix (review #2 / MAT-10 follow-up): button is gated on
    // `statusLoaded` so a user can't click before /v1/setup/status
    // resolves; but Enter-in-password sidesteps that gate, and on a
    // flaky uplink the GET might still be in flight when the user
    // submits. await fetchStatus() here as a belt + braces — it's
    // a no-op cache hit if the response already came back.
    let effectiveHubId = hubId;
    if (effectiveHubId === null) {
      const fetched = await fetchStatus();
      effectiveHubId = fetched ?? hubId;
    }
    if (effectiveHubId === null) {
      // We genuinely couldn't get the hub_id. Use the dev fallback —
      // this still works for the dev seed (hub_id=1) and gives the
      // user a real 403 if they're on a snowflake-id deployment with
      // an unreachable backend (better than infinite spinner).
      effectiveHubId = FALLBACK_HUB_ID;
    }

    try {
      const res = await fetch(`${HUB_API}/v1/auth/login`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          login: loginField,
          password,
          hub_id: effectiveHubId,
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

          {statusError && (
            // Non-blocking: form still submits (with FALLBACK_HUB_ID), but
            // the operator sees an early signal that the backend isn't
            // responsive. Amber, not red — login itself isn't broken yet,
            // it might still go through.
            <p className="rounded border border-amber-900/30 bg-amber-950/20 px-3 py-2 font-mono text-xs text-amber-300">
              {t(
                "Hub identifier unavailable — check your connection to the server",
              )}
            </p>
          )}

          {error && (
            <p className="rounded border border-red-900/30 bg-red-950/20 px-3 py-2 font-mono text-xs text-red-400">
              {error}
            </p>
          )}

          <Button
            type="submit"
            // Disable until /v1/setup/status has resolved, so a fast clicker
            // can't fire off a login with `hubId === null` (which would
            // send FALLBACK="1" and produce the exact 403 this PR fixes).
            // Enter-in-password still works because handleSubmit awaits
            // fetchStatus itself — see comment there.
            disabled={loading || !statusLoaded}
            className="mt-1 w-full bg-blue-600 font-mono text-sm tracking-wide text-white hover:bg-blue-500 disabled:opacity-40 transition-colors"
            size="lg"
          >
            {loading ? (
              <span className="flex items-center gap-2">
                <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-white/30 border-t-white" />
                {t("Logging in...")}
              </span>
            ) : !statusLoaded ? (
              <span className="flex items-center gap-2">
                <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-white/30 border-t-white" />
                {t("Preparing...")}
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
