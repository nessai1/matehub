"use client";

import { useState } from "react";
import { Bug, Check, ChevronDown, ChevronUp, Copy, Eraser } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { useVideoDebug } from "@/hooks/use-video-debug";
import type { VideoClient } from "../../../packages/sdk-video/src";

interface CallDebugPanelProps {
  client: VideoClient | null;
}

export function CallDebugPanel({ client }: CallDebugPanelProps) {
  const { logs, diagnostics, copyToClipboard, clear } = useVideoDebug(client);
  const [collapsed, setCollapsed] = useState(false);
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    const ok = await copyToClipboard();
    if (ok) {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }
  };

  if (!client) return null;

  return (
    <div
      className={cn(
        "fixed bottom-4 right-4 z-50 w-[420px] max-w-[90vw] rounded-lg border bg-card shadow-lg",
        "flex flex-col",
        collapsed ? "h-auto" : "max-h-[70vh]",
      )}
    >
      <div className="flex h-9 shrink-0 items-center gap-2 border-b px-2">
        <Bug className="h-4 w-4 text-amber-500" />
        <span className="text-xs font-semibold">Video Debug</span>
        {diagnostics?.userId && (
          <span className="rounded bg-primary/10 px-1.5 py-0.5 font-mono text-[10px] text-primary">
            {diagnostics.userId}
          </span>
        )}
        <StatePills diagnostics={diagnostics} />
        <div className="ml-auto flex items-center gap-1">
          <Button
            size="icon"
            variant="ghost"
            className="h-6 w-6"
            onClick={handleCopy}
            title="Copy JSON snapshot"
          >
            {copied ? (
              <Check className="h-3.5 w-3.5 text-emerald-500" />
            ) : (
              <Copy className="h-3.5 w-3.5" />
            )}
          </Button>
          <Button
            size="icon"
            variant="ghost"
            className="h-6 w-6"
            onClick={clear}
            title="Clear logs"
          >
            <Eraser className="h-3.5 w-3.5" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            className="h-6 w-6"
            onClick={() => setCollapsed((c) => !c)}
            title={collapsed ? "Expand" : "Collapse"}
          >
            {collapsed ? (
              <ChevronUp className="h-3.5 w-3.5" />
            ) : (
              <ChevronDown className="h-3.5 w-3.5" />
            )}
          </Button>
        </div>
      </div>

      {!collapsed && (
        <>
          <DebugSummary diagnostics={diagnostics} />
          <DebugLogFeed logs={logs} />
        </>
      )}
    </div>
  );
}

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
      ? "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400"
      : value === "failed" || value === "closed"
        ? "bg-red-500/15 text-red-600 dark:text-red-400"
        : value === "disconnected"
          ? "bg-amber-500/15 text-amber-600 dark:text-amber-400"
          : "bg-muted text-muted-foreground";
  return (
    <span
      className={cn(
        "rounded px-1.5 py-0.5 text-[10px] font-mono uppercase",
        tone,
      )}
    >
      {label}:{value}
    </span>
  );
}

function DebugSummary({
  diagnostics,
}: {
  diagnostics: ReturnType<typeof useVideoDebug>["diagnostics"];
}) {
  if (!diagnostics) {
    return (
      <div className="border-b p-2 text-xs text-muted-foreground">
        waiting for diagnostics…
      </div>
    );
  }

  return (
    <div className="grid grid-cols-2 gap-x-3 gap-y-1 border-b p-2 text-xs">
      <Row label="participant" value={shortId(diagnostics.participantId)} />
      <Row
        label="joined"
        value={diagnostics.joined ? "yes" : "no"}
      />
      <Row
        label="mic"
        value={diagnostics.micEnabled ? "on" : "off"}
      />
      <Row
        label="cam"
        value={diagnostics.camEnabled ? "on" : "off"}
      />
      <Row label="signaling" value={diagnostics.signalingState} />
      <Row label="gathering" value={diagnostics.iceGatheringState} />
      <Row
        label="local sdp"
        value={diagnostics.localDescriptionType ?? "—"}
      />
      <Row
        label="remote sdp"
        value={diagnostics.remoteDescriptionType ?? "—"}
      />
      <div className="col-span-2 mt-1 flex flex-wrap items-center gap-1">
        <span className="text-[10px] uppercase text-muted-foreground">
          senders
        </span>
        {diagnostics.senders.length === 0 && (
          <span className="text-[10px] text-muted-foreground">none</span>
        )}
        {diagnostics.senders.map((s, i) => (
          <TrackPill key={`s${i}`} info={s} />
        ))}
      </div>
      <div className="col-span-2 flex flex-wrap items-center gap-1">
        <span className="text-[10px] uppercase text-muted-foreground">
          receivers
        </span>
        {diagnostics.receivers.length === 0 && (
          <span className="text-[10px] text-muted-foreground">none</span>
        )}
        {diagnostics.receivers.map((r, i) => (
          <TrackPill key={`r${i}`} info={r} />
        ))}
      </div>
      <div className="col-span-2 mt-1 flex flex-wrap items-center gap-1">
        <span className="text-[10px] uppercase text-muted-foreground">
          participants ({diagnostics.participants.length})
        </span>
        {diagnostics.participants.map((p) => (
          <Badge
            key={p.participantId}
            variant="outline"
            className="h-5 gap-1 font-mono text-[10px]"
          >
            {p.userId}
            <span
              className={cn(
                "inline-block h-1.5 w-1.5 rounded-full",
                p.hasAudio ? "bg-emerald-500" : "bg-zinc-400",
              )}
              title="audio track attached"
            />
            <span
              className={cn(
                "inline-block h-1.5 w-1.5 rounded-full",
                p.hasVideo ? "bg-emerald-500" : "bg-zinc-400",
              )}
              title="video track attached"
            />
          </Badge>
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
      className={cn(
        "rounded border px-1 py-0.5 font-mono text-[10px]",
        live ? "border-emerald-500/40" : "border-zinc-400/40",
      )}
    >
      {info.kind ?? "?"}:{info.enabled ? "on" : "off"}
      {info.muted ? "·mut" : ""}·{info.readyState ?? "?"}
    </span>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline gap-2 font-mono">
      <span className="text-[10px] uppercase text-muted-foreground">
        {label}
      </span>
      <span className="truncate">{value}</span>
    </div>
  );
}

function DebugLogFeed({
  logs,
}: {
  logs: ReturnType<typeof useVideoDebug>["logs"];
}) {
  return (
    <ScrollArea className="flex-1 overflow-y-auto">
      <div className="flex flex-col-reverse gap-0.5 p-2 font-mono text-[11px] leading-tight">
        {logs.length === 0 && (
          <div className="text-muted-foreground">no events yet</div>
        )}
        {logs.map((entry, i) => (
          <div
            key={`${entry.time}-${i}`}
            className={cn(
              "flex gap-2 rounded px-1 py-0.5",
              entry.level === "error" && "bg-red-500/10 text-red-600",
              entry.level === "warn" && "bg-amber-500/10",
            )}
          >
            <span className="shrink-0 text-muted-foreground">
              {fmtTime(entry.time)}
            </span>
            <span className="shrink-0 font-semibold">{entry.type}</span>
            <span className="truncate" title={entry.msg}>
              {entry.msg}
            </span>
          </div>
        ))}
      </div>
    </ScrollArea>
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
