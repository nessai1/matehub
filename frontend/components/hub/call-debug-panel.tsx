import { useCallback, useEffect, useRef, useState } from "react";
import { Bug, Check, Copy, Eraser, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { useVideoDebug } from "@/hooks/use-video-debug";
import type { VideoClient } from "../../../packages/sdk-video/src";

interface CallDebugPanelProps {
  client: VideoClient | null;
}

// ── Position persistence (tracks FAB corner and panel offset) ───────────────

interface PanelPos {
  // X/Y of the draggable FAB, measured from the viewport top-left.
  // When the panel is open, the panel is anchored to the FAB's rect.
  x: number;
  y: number;
  open: boolean;
}

const STORAGE_KEY = "matehub_debug_fab";

function readPos(): PanelPos {
  if (typeof window === "undefined")
    return { x: -1, y: -1, open: false }; // sentinel → compute default
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { x: -1, y: -1, open: false };
    const p = JSON.parse(raw) as Partial<PanelPos>;
    return {
      x: typeof p.x === "number" ? p.x : -1,
      y: typeof p.y === "number" ? p.y : -1,
      open: !!p.open,
    };
  } catch {
    return { x: -1, y: -1, open: false };
  }
}

function writePos(p: PanelPos) {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(p));
  } catch {
    /* ignore */
  }
}

// Clamp point to visible viewport so the FAB doesn't end up off-screen after
// a resize or a saved position from a bigger monitor.
function clamp(x: number, y: number, fabSize = 44) {
  const w = window.innerWidth - fabSize - 8;
  const h = window.innerHeight - fabSize - 8;
  return {
    x: Math.max(8, Math.min(w, x)),
    y: Math.max(8, Math.min(h, y)),
  };
}

// ── Root ────────────────────────────────────────────────────────────────────

const FAB_SIZE = 44;
const PANEL_W = 460;
const PANEL_H = 520;

export function CallDebugPanel({ client }: CallDebugPanelProps) {
  const { logs, diagnostics, copyToClipboard, clear } = useVideoDebug(client);

  const [pos, setPos] = useState<PanelPos>({ x: -1, y: -1, open: false });
  const [copied, setCopied] = useState(false);

  // Hydrate position + track window resizes.
  // The initial setPos here is unavoidable — we can't read localStorage during
  // render (SSR) and the default "bottom-right corner" depends on innerWidth/
  // innerHeight which only exist on the client. One-shot sync, not a loop.
  useEffect(() => {
    const stored = readPos();
    const next =
      stored.x < 0 || stored.y < 0
        ? clamp(
            window.innerWidth - FAB_SIZE - 24,
            window.innerHeight - FAB_SIZE - 24,
          )
        : clamp(stored.x, stored.y);
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setPos({ x: next.x, y: next.y, open: stored.open });
    const onResize = () => {
      setPos((p) => {
        const c = clamp(p.x, p.y);
        return { x: c.x, y: c.y, open: p.open };
      });
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // Persist whenever position changes.
  useEffect(() => {
    if (pos.x >= 0 && pos.y >= 0) writePos(pos);
  }, [pos]);

  const handleCopy = useCallback(async () => {
    const ok = await copyToClipboard();
    if (ok) {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }
  }, [copyToClipboard]);

  // ── Drag handling (shared between FAB and panel header) ──────────────────

  const dragRef = useRef<{
    startX: number;
    startY: number;
    origX: number;
    origY: number;
    moved: boolean;
  } | null>(null);

  const beginDrag = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      dragRef.current = {
        startX: e.clientX,
        startY: e.clientY,
        origX: pos.x,
        origY: pos.y,
        moved: false,
      };
    },
    [pos.x, pos.y],
  );

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      const d = dragRef.current;
      if (!d) return;
      const dx = e.clientX - d.startX;
      const dy = e.clientY - d.startY;
      if (!d.moved && dx * dx + dy * dy > 9) d.moved = true;
      if (!d.moved) return;
      const c = clamp(d.origX + dx, d.origY + dy);
      setPos((p) => ({ ...p, x: c.x, y: c.y }));
    };
    const onUp = () => {
      dragRef.current = null;
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, []);

  const onFabClick = () => {
    // Click-not-drag opens the panel.
    if (dragRef.current?.moved) return;
    setPos((p) => ({ ...p, open: !p.open }));
  };

  if (!client) return null;
  if (pos.x < 0 || pos.y < 0) return null; // pre-hydration

  // Decide which corner to grow the panel into so it never overflows the
  // viewport, regardless of where the FAB was dragged to.
  const growRight = pos.x + PANEL_W + 8 < window.innerWidth;
  const growDown = pos.y + PANEL_H + 8 < window.innerHeight;
  const panelLeft = growRight ? pos.x : pos.x + FAB_SIZE - PANEL_W;
  const panelTop = growDown ? pos.y + FAB_SIZE + 8 : pos.y - PANEL_H - 8;

  return (
    <>
      {/* ── Floating FAB ──────────────────────────────────────────────── */}
      <button
        type="button"
        onMouseDown={beginDrag}
        onClick={onFabClick}
        className={cn(
          "fixed z-[60] grid place-items-center rounded-full shadow-xl transition-shadow",
          "text-amber-400 hover:shadow-2xl",
        )}
        style={{
          left: pos.x,
          top: pos.y,
          width: FAB_SIZE,
          height: FAB_SIZE,
          background: "rgba(15, 17, 25, 0.92)",
          border: "1px solid rgba(255,255,255,0.12)",
          backdropFilter: "blur(8px)",
          cursor: "grab",
        }}
        title="Video debug (drag to move, click to toggle)"
        aria-label="Toggle video debug panel"
      >
        <Bug className="h-5 w-5" />
      </button>

      {/* ── Panel ────────────────────────────────────────────────────────── */}
      {pos.open && (
        <div
          className="fixed z-[59] flex flex-col overflow-hidden rounded-xl shadow-2xl"
          style={{
            left: panelLeft,
            top: panelTop,
            width: PANEL_W,
            height: PANEL_H,
            background: "rgb(17, 19, 27)",
            color: "rgb(226, 232, 240)",
            border: "1px solid rgba(255, 255, 255, 0.08)",
          }}
        >
          <PanelHeader
            diagnostics={diagnostics}
            onDragHeader={beginDrag}
            onCopy={handleCopy}
            copied={copied}
            onClear={clear}
            onClose={() => setPos((p) => ({ ...p, open: false }))}
          />
          <DebugSummary diagnostics={diagnostics} />
          <DebugLogFeed logs={logs} />
        </div>
      )}
    </>
  );
}

// ── Panel header (draggable too) ───────────────────────────────────────────

function PanelHeader({
  diagnostics,
  onDragHeader,
  onCopy,
  copied,
  onClear,
  onClose,
}: {
  diagnostics: ReturnType<typeof useVideoDebug>["diagnostics"];
  onDragHeader: (e: React.MouseEvent) => void;
  onCopy: () => void;
  copied: boolean;
  onClear: () => void;
  onClose: () => void;
}) {
  return (
    <div
      onMouseDown={onDragHeader}
      className="flex h-10 shrink-0 select-none items-center gap-2 px-3"
      style={{
        borderBottom: "1px solid rgba(255,255,255,0.08)",
        cursor: "grab",
        background: "rgba(255,255,255,0.02)",
      }}
    >
      <Bug className="h-4 w-4 text-amber-400" />
      <span className="text-xs font-semibold tracking-wide">Video Debug</span>
      {diagnostics?.userId && (
        <span
          className="rounded px-1.5 py-0.5 font-mono text-[10px]"
          style={{
            background: "rgba(99, 102, 241, 0.18)",
            color: "rgb(165, 180, 252)",
          }}
        >
          {diagnostics.userId}
        </span>
      )}
      <StatePills diagnostics={diagnostics} />
      <div className="ml-auto flex items-center gap-1" onMouseDown={(e) => e.stopPropagation()}>
        <IconBtn onClick={onCopy} title="Copy JSON snapshot">
          {copied ? (
            <Check className="h-3.5 w-3.5 text-emerald-400" />
          ) : (
            <Copy className="h-3.5 w-3.5" />
          )}
        </IconBtn>
        <IconBtn onClick={onClear} title="Clear logs">
          <Eraser className="h-3.5 w-3.5" />
        </IconBtn>
        <IconBtn onClick={onClose} title="Close">
          <X className="h-3.5 w-3.5" />
        </IconBtn>
      </div>
    </div>
  );
}

function IconBtn({
  children,
  onClick,
  title,
}: {
  children: React.ReactNode;
  onClick: () => void;
  title: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className="grid h-6 w-6 place-items-center rounded-md text-slate-300 transition-colors hover:bg-white/10 hover:text-white"
    >
      {children}
    </button>
  );
}

// ── State pills ────────────────────────────────────────────────────────────

function StatePills({
  diagnostics,
}: {
  diagnostics: ReturnType<typeof useVideoDebug>["diagnostics"];
}) {
  if (!diagnostics) return null;
  return (
    <div className="flex items-center gap-1">
      <StatePill label="pc" value={diagnostics.pcConnectionState} />
      <StatePill label="ice" value={diagnostics.iceConnectionState} />
    </div>
  );
}

function StatePill({ label, value }: { label: string; value: string }) {
  const tone =
    value === "connected" || value === "completed"
      ? { bg: "rgba(16, 185, 129, 0.18)", fg: "rgb(110, 231, 183)" }
      : value === "failed" || value === "closed"
        ? { bg: "rgba(239, 68, 68, 0.18)", fg: "rgb(252, 165, 165)" }
        : value === "disconnected"
          ? { bg: "rgba(245, 158, 11, 0.18)", fg: "rgb(252, 211, 77)" }
          : { bg: "rgba(255,255,255,0.06)", fg: "rgb(148, 163, 184)" };
  return (
    <span
      className="rounded px-1.5 py-0.5 font-mono text-[10px] uppercase"
      style={{ background: tone.bg, color: tone.fg }}
    >
      {label}:{value}
    </span>
  );
}

// ── Summary grid ───────────────────────────────────────────────────────────

function DebugSummary({
  diagnostics,
}: {
  diagnostics: ReturnType<typeof useVideoDebug>["diagnostics"];
}) {
  if (!diagnostics) {
    return (
      <div
        className="p-2 text-xs text-slate-400"
        style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}
      >
        waiting for diagnostics…
      </div>
    );
  }
  return (
    <div
      className="grid shrink-0 grid-cols-2 gap-x-3 gap-y-1 p-2 text-xs"
      style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}
    >
      <Row label="participant" value={shortId(diagnostics.participantId)} />
      <Row label="joined" value={diagnostics.joined ? "yes" : "no"} />
      <Row label="mic" value={diagnostics.micEnabled ? "on" : "off"} />
      <Row label="cam" value={diagnostics.camEnabled ? "on" : "off"} />
      <Row label="signaling" value={diagnostics.signalingState} />
      <Row label="gathering" value={diagnostics.iceGatheringState} />
      <Row label="local sdp" value={diagnostics.localDescriptionType ?? "—"} />
      <Row
        label="remote sdp"
        value={diagnostics.remoteDescriptionType ?? "—"}
      />
      <div className="col-span-2 mt-1 flex flex-wrap items-center gap-1">
        <span className="text-[10px] uppercase text-slate-400">senders</span>
        {diagnostics.senders.length === 0 && (
          <span className="text-[10px] text-slate-500">none</span>
        )}
        {diagnostics.senders.map((s, i) => (
          <TrackPill key={`s${i}`} info={s} />
        ))}
      </div>
      <div className="col-span-2 flex flex-wrap items-center gap-1">
        <span className="text-[10px] uppercase text-slate-400">receivers</span>
        {diagnostics.receivers.length === 0 && (
          <span className="text-[10px] text-slate-500">none</span>
        )}
        {diagnostics.receivers.map((r, i) => (
          <TrackPill key={`r${i}`} info={r} />
        ))}
      </div>
      <div className="col-span-2 mt-1 flex flex-wrap items-center gap-1">
        <span className="text-[10px] uppercase text-slate-400">
          participants ({diagnostics.participants.length})
        </span>
        {diagnostics.participants.map((p) => (
          <span
            key={p.participantId}
            className="inline-flex h-5 items-center gap-1 rounded px-1.5 font-mono text-[10px]"
            style={{
              border: "1px solid rgba(255,255,255,0.12)",
              color: "rgb(226, 232, 240)",
            }}
          >
            {p.userId}
            <span
              className="inline-block h-1.5 w-1.5 rounded-full"
              style={{
                background: p.hasAudio
                  ? "rgb(16, 185, 129)"
                  : "rgb(100, 116, 139)",
              }}
              title="audio track attached"
            />
            <span
              className="inline-block h-1.5 w-1.5 rounded-full"
              style={{
                background: p.hasVideo
                  ? "rgb(16, 185, 129)"
                  : "rgb(100, 116, 139)",
              }}
              title="video track attached"
            />
          </span>
        ))}
      </div>
    </div>
  );
}

function TrackPill({
  info,
}: {
  info: {
    kind: string | undefined;
    enabled: boolean | undefined;
    muted: boolean | undefined;
    readyState: string | undefined;
  };
}) {
  const live = info.readyState === "live";
  return (
    <span
      className="rounded px-1 py-0.5 font-mono text-[10px]"
      style={{
        border: live
          ? "1px solid rgba(16, 185, 129, 0.5)"
          : "1px solid rgba(255,255,255,0.12)",
      }}
    >
      {info.kind ?? "?"}:{info.enabled ? "on" : "off"}
      {info.muted ? "·mut" : ""}·{info.readyState ?? "?"}
    </span>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline gap-2 font-mono">
      <span className="text-[10px] uppercase text-slate-400">{label}</span>
      <span className="truncate text-slate-100">{value}</span>
    </div>
  );
}

// ── Log feed ───────────────────────────────────────────────────────────────

function DebugLogFeed({
  logs,
}: {
  logs: ReturnType<typeof useVideoDebug>["logs"];
}) {
  return (
    <div className="flex-1 overflow-y-auto">
      <div className="flex flex-col-reverse gap-0.5 p-2 font-mono text-[11px] leading-tight">
        {logs.length === 0 && (
          <div className="text-slate-500">no events yet</div>
        )}
        {logs.map((entry, i) => (
          <div
            key={`${entry.time}-${i}`}
            className="flex gap-2 rounded px-1 py-0.5"
            style={{
              background:
                entry.level === "error"
                  ? "rgba(239, 68, 68, 0.12)"
                  : entry.level === "warn"
                    ? "rgba(245, 158, 11, 0.1)"
                    : "transparent",
              color:
                entry.level === "error"
                  ? "rgb(252, 165, 165)"
                  : "rgb(226, 232, 240)",
            }}
          >
            <span className="shrink-0 text-slate-500">
              {fmtTime(entry.time)}
            </span>
            <span className="shrink-0 font-semibold">{entry.type}</span>
            <span className="truncate" title={entry.msg}>
              {entry.msg}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function fmtTime(ms: number): string {
  const d = new Date(ms);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  const fff = String(d.getMilliseconds()).padStart(3, "0");
  return `${hh}:${mm}:${ss}.${fff}`;
}

function shortId(id: string | null): string {
  if (!id) return "—";
  return id.length > 8 ? `${id.slice(0, 8)}…` : id;
}
