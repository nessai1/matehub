import { useCallback, useEffect, useRef, useState } from "react";
import type { VideoClient, VideoDiagnostics } from "../../packages/sdk-video/src";

export interface DebugLogEntry {
  time: number;
  level: "info" | "warn" | "error";
  type: string;
  msg: string;
  data?: unknown;
}

const MAX_LOG_ENTRIES = 500;
const DIAGNOSTICS_POLL_MS = 1000;

/** Self-describing debug snapshot — identical shape whether copied to the
 *  clipboard or uploaded to the server's ROOM_DEBUG sink. */
export interface DebugBundle {
  userId: string | null;
  sessionId: string | null;
  participantId: string | null;
  timestamp: string;
  userAgent: string | null;
  diagnostics: VideoDiagnostics | null;
  logs: DebugLogEntry[];
}

interface UseVideoDebugReturn {
  logs: DebugLogEntry[];
  diagnostics: VideoDiagnostics | null;
  /** Assemble the current {identity, diagnostics, logs} snapshot. */
  buildBundle: () => DebugBundle;
  /** Dump the snapshot as JSON into clipboard. */
  copyToClipboard: () => Promise<boolean>;
  /** Wipe the collected log ring buffer. */
  clear: () => void;
}

/**
 * Collects debug events from VideoClient plus periodic getDiagnostics() snapshots.
 * Dev-only companion to the call UI — all state lives in memory, no network.
 */
export function useVideoDebug(client: VideoClient | null): UseVideoDebugReturn {
  const [logs, setLogs] = useState<DebugLogEntry[]>([]);
  const [diagnostics, setDiagnostics] = useState<VideoDiagnostics | null>(null);
  // Ref mirror of logs so copyToClipboard reads the latest without closure staleness.
  const logsRef = useRef<DebugLogEntry[]>([]);
  const diagRef = useRef<VideoDiagnostics | null>(null);

  const append = useCallback((entry: DebugLogEntry) => {
    logsRef.current = [...logsRef.current.slice(-(MAX_LOG_ENTRIES - 1)), entry];
    setLogs(logsRef.current);
  }, []);

  useEffect(() => {
    if (!client) return;

    const unsubscribe = client.on((event) => {
      let entry: DebugLogEntry;
      if (event.type === "debug") {
        entry = {
          time: Date.now(),
          level: event.level,
          type: "debug",
          msg: event.msg,
          data: event.data,
        };
      } else {
        // Every high-level event gets a record too — useful for seeing
        // the sequence of track_added / participant_joined / muted etc.
        entry = {
          time: Date.now(),
          level: event.type === "error" ? "error" : "info",
          type: event.type,
          msg: event.type,
          data: sanitizeEvent(event),
        };
      }
      append(entry);
    });

    let cancelled = false;
    const poll = async () => {
      if (cancelled) return;
      try {
        const d = await client.getDiagnostics();
        if (!cancelled) {
          diagRef.current = d;
          setDiagnostics(d);
        }
      } catch (e) {
        // getDiagnostics shouldn't throw, but getStats can on detached PCs.
        // Swallow — debug panel keeps its last good snapshot.
        console.warn("[useVideoDebug] getDiagnostics failed", e);
      }
    };
    poll();
    const interval = setInterval(poll, DIAGNOSTICS_POLL_MS);

    return () => {
      cancelled = true;
      clearInterval(interval);
      unsubscribe();
    };
  }, [client, append]);

  const buildBundle = useCallback((): DebugBundle => {
    const d = diagRef.current;
    return {
      // Hoist identity to the top so a pasted/uploaded dump answers "whose is
      // this?" before you even scroll.
      userId: d?.userId ?? null,
      sessionId: d?.sessionId ?? null,
      participantId: d?.participantId ?? null,
      timestamp: new Date().toISOString(),
      userAgent: typeof navigator !== "undefined" ? navigator.userAgent : null,
      diagnostics: d,
      logs: logsRef.current,
    };
  }, []);

  const copyToClipboard = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(JSON.stringify(buildBundle(), null, 2));
      return true;
    } catch (e) {
      console.error("[useVideoDebug] clipboard copy failed", e);
      return false;
    }
  }, [buildBundle]);

  const clear = useCallback(() => {
    logsRef.current = [];
    setLogs([]);
  }, []);

  return { logs, diagnostics, buildBundle, copyToClipboard, clear };
}

// MediaStreamTrack / MediaStream don't serialize — strip them down to ids
// so the JSON payload is useful when pasted.
function sanitizeEvent(event: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(event)) {
    if (v instanceof MediaStreamTrack) {
      out[k] = {
        _kind: "MediaStreamTrack",
        id: v.id,
        kind: v.kind,
        enabled: v.enabled,
        muted: v.muted,
        readyState: v.readyState,
      };
    } else if (v instanceof MediaStream) {
      out[k] = {
        _kind: "MediaStream",
        id: v.id,
        tracks: v.getTracks().map((t) => ({ id: t.id, kind: t.kind })),
      };
    } else if (
      v &&
      typeof v === "object" &&
      "participantId" in v &&
      "userId" in v
    ) {
      // Participant — keep only the scalar fields.
      const p = v as Record<string, unknown>;
      out[k] = {
        participantId: p.participantId,
        userId: p.userId,
        hasAudio: !!p.audioTrack,
        hasVideo: !!p.videoTrack,
        isSpeaking: p.isSpeaking,
        isMicMuted: p.isMicMuted,
      };
    } else {
      out[k] = v;
    }
  }
  return out;
}
