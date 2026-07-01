import { useEffect } from "react";
import { useVideoDebug } from "@/hooks/use-video-debug";
import type { VideoClient } from "@matehub/sdk-video";

/** How often each client ships its debug snapshot while a call is active.
 *  Matches the cadence the server-side log is written at, so the per-call
 *  bundle shows how the call evolves (artifacts usually build up mid-call). */
const UPLOAD_INTERVAL_MS = 5000;

interface DebugUploaderProps {
  client: VideoClient | null;
  sessionId: string | null;
  /** Video service base URL (same one used for session create / WS). */
  serverUrl: string;
  /** Only true when the server reported `debug_capture` on session create
   *  (i.e. it runs with ROOM_DEBUG=1). Off → this renders nothing and never
   *  collects or uploads, so a normal deployment pays zero cost. */
  enabled: boolean;
}

/**
 * Headless companion to a call: while ROOM_DEBUG capture is enabled on the
 * server, periodically POSTs this client's debug bundle (diagnostics + log
 * ring buffer) to `/v1/sessions/{id}/debug`, plus a final flush when the call
 * ends or the tab is closed.
 *
 * Lives at the call-provider level (not inside the debug panel) so capture
 * runs whether or not anyone opened the panel. Renders `null`.
 */
export function DebugUploader({
  client,
  sessionId,
  serverUrl,
  enabled,
}: DebugUploaderProps) {
  // Passing `null` when disabled makes the hook a no-op — no event
  // subscription, no getStats polling.
  const active = enabled && !!client && !!sessionId;
  const { buildBundle } = useVideoDebug(active ? client : null);

  useEffect(() => {
    if (!active || !sessionId) return;

    const url = `${serverUrl}/v1/sessions/${sessionId}/debug`;
    const post = (keepalive: boolean) => {
      let body: string;
      try {
        body = JSON.stringify(buildBundle());
      } catch {
        return;
      }
      // Fire-and-forget. `keepalive` lets the final flush survive teardown /
      // tab close. Errors are swallowed — debug upload must never disrupt a
      // call (and a 404 just means capture was turned off server-side).
      void fetch(url, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body,
        keepalive,
      }).catch(() => {});
    };

    const interval = setInterval(() => post(false), UPLOAD_INTERVAL_MS);
    return () => {
      clearInterval(interval);
      post(true);
    };
  }, [active, sessionId, serverUrl, buildBundle]);

  return null;
}
