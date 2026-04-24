import { FormEvent, useState } from "react";
import { useNavigate } from "react-router";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useAuth } from "@/lib/auth";

const HUB_API = import.meta.env.VITE_HUB_API_URL || "http://localhost:3002";
// TODO: get hub_id from subdomain or config. Wire-format is always a string
// because Snowflake IDs blow past JS MAX_SAFE_INTEGER.
const DEV_HUB_ID = "1";

export default function LoginPage() {
  const navigate = useNavigate();
  const { login } = useAuth();
  const [loginField, setLoginField] = useState("");
  const [password, setPassword] = useState("");
  const [rememberMe, setRememberMe] = useState(false);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setLoading(true);

    try {
      const res = await fetch(`${HUB_API}/v1/auth/login`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ login: loginField, password, hub_id: DEV_HUB_ID, remember_me: rememberMe }),
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
          Sign in
        </h1>
        <p className="mt-1 font-mono text-xs text-zinc-500">
          Dev Hub
        </p>
      </div>

      <div className="w-full rounded-lg border border-zinc-800 bg-zinc-900/80 p-8 backdrop-blur-sm">
        <form onSubmit={handleSubmit} className="flex flex-col gap-5">
          <div className="flex flex-col gap-2">
            <label className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase">
              Username or Email
            </label>
            <Input
              type="text"
              value={loginField}
              onChange={(e) => setLoginField(e.target.value)}
              placeholder="alice or alice@matehub.dev"
              required
              autoFocus
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-indigo-500/30"
            />
          </div>

          <div className="flex flex-col gap-2">
            <label className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase">
              Password
            </label>
            <Input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="--------"
              required
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-indigo-500/30"
            />
          </div>

          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={rememberMe}
              onChange={(e) => setRememberMe(e.target.checked)}
              className="h-3.5 w-3.5 rounded border-zinc-700 bg-zinc-950 text-indigo-500 focus:ring-indigo-500/30"
            />
            <span className="font-mono text-xs text-zinc-400">Remember me</span>
          </label>

          {error && (
            <p className="rounded border border-red-900/30 bg-red-950/20 px-3 py-2 font-mono text-xs text-red-400">
              {error}
            </p>
          )}

          <Button
            type="submit"
            disabled={loading}
            className="mt-1 w-full bg-indigo-600 font-mono text-sm tracking-wide text-white hover:bg-indigo-500 disabled:opacity-40 transition-colors"
            size="lg"
          >
            {loading ? (
              <span className="flex items-center gap-2">
                <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-white/30 border-t-white" />
                Signing in
              </span>
            ) : (
              "Sign in"
            )}
          </Button>
        </form>
      </div>
    </div>
  );
}
