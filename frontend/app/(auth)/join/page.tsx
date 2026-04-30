import { useCallback, useEffect, useState } from "react";
import { useParams, useNavigate } from "react-router";
import { Button } from "@/components/ui/button";
import { useAuth } from "@/lib/auth";
import { t } from "@/i18n";

interface JoinData {
  hub_id: string;
  hub_name: string;
  hub_slug: string;
  temp_user_id: string;
  nickname: string;
  session_token: string;
}

type Phase = "resolving" | "ready" | "entering" | "error";

export default function JoinPage() {
  const params = useParams<{ token: string }>();
  const navigate = useNavigate();
  const { login } = useAuth();

  const [phase, setPhase] = useState<Phase>("resolving");
  const [data, setData] = useState<JoinData | null>(null);
  const [error, setError] = useState("");
  const [dots, setDots] = useState("");

  // Animated dots during resolve
  useEffect(() => {
    if (phase !== "resolving") return;
    const iv = setInterval(() => {
      setDots((d) => (d.length >= 3 ? "" : d + "."));
    }, 400);
    return () => clearInterval(iv);
  }, [phase]);

  // Resolve token
  useEffect(() => {
    const resolve = async () => {
      try {
        const res = await fetch(`/api/hub/v1/join/${params.token}`);
        if (!res.ok) {
          if (res.status === 404) {
            setError(t("Link expired or revoked"));
          } else {
            setError(`${t("Server error")} (${res.status})`);
          }
          setPhase("error");
          return;
        }
        const json: JoinData = await res.json();
        // Small delay so the resolve animation is visible
        await new Promise((r) => setTimeout(r, 600));
        setData(json);
        setPhase("ready");
      } catch {
        setError(t("Cannot reach server"));
        setPhase("error");
      }
    };
    resolve();
  }, [params.token]);

  const enter = useCallback(() => {
    if (!data) return;
    setPhase("entering");
    login({
      type: "temp",
      userId: data.temp_user_id,
      username: data.nickname,
      displayName: data.nickname,
      hubId: data.hub_id,
      hubSlug: data.hub_slug,
      token: data.session_token,
    });
    navigate("/hub");
  }, [data, login, navigate]);

  return (
    <div className="flex flex-col items-center">
      {/* Token display */}
      <div className="mb-8 font-mono text-[10px] tracking-widest text-zinc-600 select-all break-all text-center">
        {params.token}
      </div>

      {/* Main card */}
      <div className="w-full rounded-lg border border-zinc-800 bg-zinc-900/80 p-8 backdrop-blur-sm">
        {phase === "resolving" && (
          <div className="flex flex-col items-center gap-4 animate-in fade-in duration-300">
            <div className="flex h-12 w-12 items-center justify-center rounded-full border border-zinc-700">
              <svg
                className="h-5 w-5 animate-spin text-zinc-500"
                viewBox="0 0 24 24"
                fill="none"
              >
                <circle
                  cx="12"
                  cy="12"
                  r="10"
                  stroke="currentColor"
                  strokeWidth="2"
                  className="opacity-20"
                />
                <path
                  d="M12 2a10 10 0 0 1 10 10"
                  stroke="currentColor"
                  strokeWidth="2"
                  strokeLinecap="round"
                />
              </svg>
            </div>
            <p className="font-mono text-sm text-zinc-400">
              {t("Resolving invite")}{dots}
            </p>
          </div>
        )}

        {phase === "ready" && data && (
          <div className="flex flex-col items-center gap-6 animate-in fade-in slide-in-from-bottom-2 duration-500">
            {/* Hub identity */}
            <div className="flex h-16 w-16 items-center justify-center rounded-xl border border-zinc-700 bg-zinc-800 text-2xl font-bold text-zinc-300">
              {data.hub_name.charAt(0).toUpperCase()}
            </div>

            <div className="text-center">
              <p className="font-mono text-[10px] tracking-widest text-blue-400/80 uppercase">
                {t("You are invited to")}
              </p>
              <h1 className="mt-1 text-2xl font-bold tracking-tight text-zinc-100">
                {data.hub_name}
              </h1>
            </div>

            <div className="w-full border-t border-zinc-800" />

            {/* Identity assignment */}
            <div className="flex w-full items-center gap-3 rounded-md border border-zinc-800 bg-zinc-950/50 px-4 py-3">
              <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-blue-500/10 text-sm font-bold text-blue-400">
                {data.nickname.charAt(0).toUpperCase()}
              </div>
              <div className="min-w-0">
                <p className="font-mono text-[10px] text-zinc-500 uppercase">
                  {t("Your identity")}
                </p>
                <p className="truncate text-sm font-medium text-zinc-200">
                  {data.nickname}
                </p>
              </div>
            </div>

            <Button
              onClick={enter}
              className="w-full bg-blue-600 font-mono text-sm tracking-wide text-white hover:bg-blue-500 transition-colors"
              size="lg"
            >
              {t("Enter Hub")}
            </Button>

            <p className="text-center font-mono text-[10px] text-zinc-600">
              {t("Temporary access. No account required.")}
            </p>
          </div>
        )}

        {phase === "entering" && (
          <div className="flex flex-col items-center gap-4 animate-in fade-in duration-300">
            <div className="h-5 w-5 animate-spin rounded-full border-2 border-blue-500 border-t-transparent" />
            <p className="font-mono text-sm text-blue-400">
              {t("Entering hub...")}
            </p>
          </div>
        )}

        {phase === "error" && (
          <div className="flex flex-col items-center gap-4 animate-in fade-in duration-300">
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
            <div className="text-center">
              <p className="text-sm font-medium text-zinc-300">{error}</p>
              <p className="mt-1 font-mono text-[10px] text-zinc-600">
                {t("Contact the person who shared this link.")}
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
