import { FormEvent, useEffect, useState } from "react";
import { useParams, useNavigate } from "react-router";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useAuth } from "@/lib/auth";

interface InviteData {
  email: string;
  hub_name: string;
  hub_id: string;
  hub_slug: string;
}

export default function RegisterPage() {
  const params = useParams<{ invite: string }>();
  const navigate = useNavigate();
  const { login } = useAuth();

  const [invite, setInvite] = useState<InviteData | null>(null);
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");

  const [username, setUsername] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [password, setPassword] = useState("");

  // Resolve invite
  useEffect(() => {
    const resolve = async () => {
      try {
        const hubApi =
          import.meta.env.VITE_HUB_API_URL || "http://localhost:3002";
        const res = await fetch(`${hubApi}/v1/invite/${params.invite}`);
        if (!res.ok) {
          setError("Invite link is invalid or expired");
          setLoading(false);
          return;
        }
        const data: InviteData = await res.json();
        setInvite(data);
      } catch {
        setError("Cannot reach server");
      }
      setLoading(false);
    };
    resolve();
  }, [params.invite]);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!invite) return;
    setError("");
    setSubmitting(true);

    try {
      const hubApi =
        import.meta.env.VITE_HUB_API_URL || "http://localhost:3002";
      const res = await fetch(`${hubApi}/v1/auth/register`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          invite_code: params.invite,
          email: invite.email,
          username,
          display_name: displayName,
          password,
        }),
      });

      if (!res.ok) {
        const body = await res.json().catch(() => null);
        setError(body?.message || "Registration failed");
        setSubmitting(false);
        return;
      }

      const data = await res.json();
      login({
        type: "permanent",
        userId: data.user_id,
        username: data.username,
        displayName: data.display_name,
        hubId: invite.hub_id,
        hubSlug: invite.hub_slug,
        token: data.token,
      });
      navigate("/hub");
    } catch {
      setError("Cannot reach server");
      setSubmitting(false);
    }
  };

  if (loading) {
    return (
      <div className="flex flex-col items-center gap-4">
        <div className="h-5 w-5 animate-spin rounded-full border-2 border-zinc-600 border-t-zinc-300" />
        <p className="font-mono text-xs text-zinc-500">
          Verifying invite...
        </p>
      </div>
    );
  }

  if (!invite) {
    return (
      <div className="w-full rounded-lg border border-zinc-800 bg-zinc-900/80 p-8 backdrop-blur-sm">
        <div className="flex flex-col items-center gap-4">
          <div className="flex h-12 w-12 items-center justify-center rounded-full border border-red-900/50 bg-red-950/30">
            <svg
              className="h-5 w-5 text-red-400"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
            >
              <path d="M18 6 6 18M6 6l12 12" />
            </svg>
          </div>
          <p className="text-sm text-zinc-300">{error}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-col items-center">
      <div className="mb-8 text-center">
        <p className="font-mono text-[10px] tracking-widest text-indigo-400/80 uppercase">
          You have been invited to
        </p>
        <h1 className="mt-1 text-2xl font-bold tracking-tight text-zinc-100">
          {invite.hub_name}
        </h1>
      </div>

      <div className="w-full rounded-lg border border-zinc-800 bg-zinc-900/80 p-8 backdrop-blur-sm">
        <p className="mb-6 text-center font-mono text-xs text-zinc-500">
          Create your account to join
        </p>

        <form onSubmit={handleSubmit} className="flex flex-col gap-5">
          {/* Email (from invite, read-only) */}
          <div className="flex flex-col gap-2">
            <label className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase">
              Email
            </label>
            <Input
              type="email"
              value={invite.email}
              disabled
              className="border-zinc-800 bg-zinc-950/30 font-mono text-sm text-zinc-500"
            />
          </div>

          <div className="flex flex-col gap-2">
            <label className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase">
              Username
            </label>
            <Input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="how others see you"
              required
              autoFocus
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-indigo-500/30"
            />
          </div>

          <div className="flex flex-col gap-2">
            <label className="font-mono text-[10px] tracking-widest text-zinc-500 uppercase">
              Display name
            </label>
            <Input
              type="text"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="your real name (optional)"
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
              minLength={8}
              className="border-zinc-800 bg-zinc-950/50 font-mono text-sm text-zinc-200 placeholder:text-zinc-700 focus-visible:ring-indigo-500/30"
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
            className="mt-1 w-full bg-indigo-600 font-mono text-sm tracking-wide text-white hover:bg-indigo-500 disabled:opacity-40 transition-colors"
            size="lg"
          >
            {submitting ? (
              <span className="flex items-center gap-2">
                <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-white/30 border-t-white" />
                Creating account
              </span>
            ) : (
              "Create account & join"
            )}
          </Button>
        </form>

        <div className="mt-6 border-t border-zinc-800 pt-4 text-center">
          <p className="font-mono text-[10px] text-zinc-600">
            Already have an account?{" "}
            <a
              href="/login"
              className="text-zinc-400 underline underline-offset-2 hover:text-zinc-200 transition-colors"
            >
              Sign in
            </a>
          </p>
        </div>
      </div>
    </div>
  );
}
