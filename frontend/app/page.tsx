import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { useAuth } from "@/lib/auth";

const HUB_API = "/api/hub";

// Boot gate. The first browser to land on a fresh hub gets routed to
// /setup; everyone else flows to /hub or /login as before.
export default function Home() {
  const { session, isLoading } = useAuth();
  const navigate = useNavigate();
  const [needsSetup, setNeedsSetup] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetch(`${HUB_API}/v1/setup/status`)
      .then((r) => r.json())
      .then((d: { needs_setup: boolean }) => {
        if (!cancelled) setNeedsSetup(d.needs_setup);
      })
      .catch(() => {
        if (!cancelled) setNeedsSetup(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (isLoading || needsSetup === null) return;
    if (needsSetup) {
      navigate("/setup", { replace: true });
      return;
    }
    navigate(session ? "/hub" : "/login", { replace: true });
  }, [session, isLoading, needsSetup, navigate]);

  return (
    <div className="flex h-screen items-center justify-center bg-background">
      <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
    </div>
  );
}
