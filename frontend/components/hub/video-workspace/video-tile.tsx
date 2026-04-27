import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { MicOffIcon, MonitorIcon, Maximize2Icon, XIcon } from "lucide-react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { cn } from "@/lib/utils";

export type TileSize = "xs" | "sm" | "md" | "lg";

export interface CameraTile {
  kind: "camera";
  id: string;
  userId: string;
  displayName: string;
  avatarUrl: string | null;
  audioTrack: MediaStreamTrack | null;
  videoTrack: MediaStreamTrack | null;
  isMicMuted: boolean;
  isSpeaking: boolean;
  isLocal: boolean;
  /** If true, shows a small pinned badge in the corner (used in spotlight). */
  pinned?: boolean;
  /** If true, shows a HOST badge (top-right). */
  host?: boolean;
  /** Outgoing-DM-call placeholder for the peer who hasn't picked up yet.
   * Renders grayscale + pulsing with a "Ringing…" caption. */
  ringing?: boolean;
}

export interface ScreenTile {
  kind: "screen";
  id: string;
  ownerUserId: string;
  ownerDisplayName: string;
  videoTrack: MediaStreamTrack;
  audioTrack: MediaStreamTrack | null;
  isLocal: boolean;
  pinned?: boolean;
}

export type Tile = CameraTile | ScreenTile;

const AVATAR_BY_SIZE: Record<TileSize, string> = {
  xs: "h-8 w-8 text-sm",
  sm: "h-11 w-11 text-base",
  md: "h-16 w-16 text-2xl",
  lg: "h-[88px] w-[88px] text-3xl",
};

const PAD_BY_SIZE: Record<TileSize, string> = {
  xs: "p-2",
  sm: "p-2.5",
  md: "p-3.5",
  lg: "p-4",
};

const NAME_FONT_BY_SIZE: Record<TileSize, string> = {
  xs: "text-[10px]",
  sm: "text-[11px]",
  md: "text-[12px]",
  lg: "text-[13px]",
};

// ── Speaking bars (mini equalizer) ─────────────────────────────────────────

function SpeakingBars() {
  return (
    <span
      className="inline-flex items-end gap-[2px]"
      style={{ height: 12 }}
      aria-hidden
    >
      {[0.5, 1, 0.7].map((h, i) => (
        <span
          key={i}
          className="animate-pulse rounded-sm"
          style={{
            width: 2.5,
            height: `${h * 12}px`,
            background: "oklch(0.78 0.18 150)",
            animationDelay: `${i * 120}ms`,
          }}
        />
      ))}
    </span>
  );
}

// ── Camera tile ────────────────────────────────────────────────────────────

function cameraTileGradient(name: string): string {
  const palettes = [
    "linear-gradient(135deg, oklch(0.38 0.12 30), oklch(0.28 0.15 20))",
    "linear-gradient(160deg, oklch(0.42 0.15 200), oklch(0.28 0.1 220))",
    "linear-gradient(140deg, oklch(0.45 0.14 145), oklch(0.3 0.1 165))",
    "linear-gradient(155deg, oklch(0.4 0.13 305), oklch(0.28 0.12 280))",
    "linear-gradient(125deg, oklch(0.42 0.14 65), oklch(0.3 0.1 50))",
    "linear-gradient(165deg, oklch(0.4 0.14 240), oklch(0.28 0.11 260))",
    "linear-gradient(135deg, oklch(0.45 0.14 340), oklch(0.3 0.1 320))",
    "linear-gradient(145deg, oklch(0.42 0.12 170), oklch(0.3 0.1 150))",
  ];
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffff;
  return palettes[h % palettes.length];
}

export function CameraVideoTile({
  tile,
  size,
  onClick,
}: {
  tile: CameraTile;
  size: TileSize;
  onClick?: () => void;
}) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const audioRef = useRef<HTMLAudioElement>(null);

  useEffect(() => {
    const el = videoRef.current;
    if (!el) return;
    if (tile.videoTrack) {
      el.srcObject = new MediaStream([tile.videoTrack]);
      el.play().catch(() => {});
    } else {
      el.srcObject = null;
    }
  }, [tile.videoTrack]);

  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    if (tile.audioTrack && !tile.isLocal) {
      el.srcObject = new MediaStream([tile.audioTrack]);
      el.play().catch(() => {});
    } else {
      el.srcObject = null;
    }
  }, [tile.audioTrack, tile.isLocal]);

  const initials = tile.displayName
    .split(" ")
    .map((n) => n[0])
    .join("")
    .toUpperCase()
    .slice(0, 2);

  const hasVideo = !!tile.videoTrack;
  const radius = size === "xs" ? "rounded-[10px]" : "rounded-[14px]";

  // Color-rich placeholder gradient so camera-off tiles aren't flat.
  const placeholderBg = cameraTileGradient(tile.displayName);

  return (
    <div
      onClick={onClick}
      className={cn(
        "group/tile relative flex h-full w-full min-h-0 cursor-pointer select-none items-center justify-center overflow-hidden transition-[box-shadow,border-color] duration-150",
        radius,
        // Outgoing-call placeholder: greyscale + soft pulse on the whole tile
        // so the inviter has a visible "we're ringing them" affordance.
        tile.ringing && "grayscale animate-pulse",
      )}
      style={{
        background: hasVideo
          ? "oklch(0.22 0.015 265)"
          : placeholderBg,
        border: tile.isSpeaking
          ? "2px solid oklch(0.78 0.18 150)"
          : "1px solid rgba(255,255,255,0.06)",
        boxShadow: tile.isSpeaking
          ? "0 0 0 3px oklch(0.78 0.18 150 / 0.18), 0 8px 32px rgba(0,0,0,0.35)"
          : "0 4px 18px rgba(0,0,0,0.25)",
      }}
    >
      {/* Video feed */}
      {hasVideo && (
        <video
          ref={videoRef}
          autoPlay
          playsInline
          muted={tile.isLocal}
          // disablePictureInPicture suppresses Chrome's auto-injected PiP
          // button that hovers over the corner. We don't expose PiP — users
          // pin / spotlight tiles via our own UI instead.
          disablePictureInPicture
          className={cn(
            "absolute inset-0 h-full w-full object-cover",
            tile.isLocal && "-scale-x-100",
          )}
        />
      )}

      {/* Camera-off → centered avatar with the placeholder gradient behind it */}
      {!hasVideo && (
        <Avatar
          className={cn(
            "ring-2 ring-white/10",
            AVATAR_BY_SIZE[size],
          )}
        >
          {tile.avatarUrl && <AvatarImage src={tile.avatarUrl} />}
          <AvatarFallback className="bg-white/10 font-semibold text-white/95 backdrop-blur">
            {initials}
          </AvatarFallback>
        </Avatar>
      )}

      {!tile.isLocal && <audio ref={audioRef} autoPlay playsInline hidden />}

      {/* Pinned badge (top-left, glass pill) */}
      {tile.pinned && size !== "xs" && (
        <div
          className={cn(
            "absolute left-3 top-3 inline-flex items-center gap-1 rounded-md px-2 py-[3px] font-mono text-[10px] font-semibold uppercase tracking-wider text-white backdrop-blur",
          )}
          style={{ background: "rgba(20,22,30,0.55)" }}
        >
          <span className="text-[9px]">📌</span>
          PINNED
        </div>
      )}

      {/* HOST badge (top-right, solid indigo) */}
      {tile.host && size !== "xs" && (
        <div
          className="absolute right-3 top-3 rounded-md px-[7px] py-[3px] font-mono text-[9px] font-bold uppercase tracking-[0.5px] text-white"
          style={{ background: "oklch(0.62 0.19 265)" }}
        >
          host
        </div>
      )}

      {/* Bottom-left name pill (glass) */}
      <div
        className={cn(
          "pointer-events-none absolute left-0 right-0 flex items-center gap-1.5",
          PAD_BY_SIZE[size],
        )}
        style={{ top: "auto", bottom: 0 }}
      >
        <div
          className={cn(
            "inline-flex max-w-[85%] items-center gap-1.5 rounded-full px-2.5 py-[4px] font-medium text-white backdrop-blur",
            NAME_FONT_BY_SIZE[size],
          )}
          style={{ background: "rgba(20,22,30,0.55)" }}
        >
          <span className="truncate">
            {tile.ringing
              ? `${tile.displayName} · Ringing…`
              : tile.isLocal
                ? `${tile.displayName} (you)`
                : tile.displayName}
          </span>
          {!tile.ringing && tile.isSpeaking && <SpeakingBars />}
          {!tile.ringing && tile.isMicMuted && (
            <MicOffIcon className="h-3 w-3 text-[oklch(0.72_0.18_25)]" />
          )}
        </div>
      </div>
    </div>
  );
}

// ── Screen-share tile ─────────────────────────────────────────────────────

export function ScreenShareVideoTile({
  tile,
  size,
  onClick,
}: {
  tile: ScreenTile;
  size: TileSize;
  onClick?: () => void;
}) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const audioRef = useRef<HTMLAudioElement>(null);
  const expandedVideoRef = useRef<HTMLVideoElement>(null);
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    const el = videoRef.current;
    if (!el) return;
    el.srcObject = new MediaStream([tile.videoTrack]);
    el.play().catch(() => {});
  }, [tile.videoTrack]);

  // Mirror the same track into the overlay <video> while expanded. Two video
  // elements consuming one MediaStreamTrack is fine — the browser pulls each
  // their own decoded frame.
  useEffect(() => {
    if (!expanded) return;
    const el = expandedVideoRef.current;
    if (!el) return;
    el.srcObject = new MediaStream([tile.videoTrack]);
    el.play().catch(() => {});
  }, [expanded, tile.videoTrack]);

  // Esc closes the overlay — keeps the muscle-memory of native fullscreen.
  useEffect(() => {
    if (!expanded) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setExpanded(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [expanded]);

  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    if (tile.audioTrack && !tile.isLocal) {
      el.srcObject = new MediaStream([tile.audioTrack]);
      el.play().catch(() => {});
    } else {
      el.srcObject = null;
    }
  }, [tile.audioTrack, tile.isLocal]);

  const radius = size === "xs" ? "rounded-[10px]" : "rounded-[14px]";

  return (
    <div
      onClick={onClick}
      className={cn(
        "group/screen relative flex h-full w-full min-h-0 cursor-pointer select-none items-center justify-center overflow-hidden bg-black",
        radius,
      )}
      style={{
        border: "1px solid rgba(255,255,255,0.08)",
        boxShadow: "0 4px 18px rgba(0,0,0,0.25)",
      }}
    >
      <video
        ref={videoRef}
        autoPlay
        playsInline
        disablePictureInPicture
        className="h-full w-full object-contain"
      />
      {tile.audioTrack && !tile.isLocal && (
        <audio ref={audioRef} autoPlay playsInline hidden />
      )}

      {/* Fullscreen button — opens our own overlay (not the native fullscreen
          API) so we control the chrome and don't get the browser PiP/exit
          banner sitting on top. */}
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          setExpanded(true);
        }}
        className="absolute right-2 top-2 rounded-md bg-black/60 p-1.5 text-white opacity-0 transition-opacity hover:bg-black/80 group-hover/screen:opacity-100"
        aria-label="Expand"
      >
        <Maximize2Icon className="h-3.5 w-3.5" />
      </button>

      {expanded &&
        typeof document !== "undefined" &&
        createPortal(
          <div
            className="fixed inset-0 z-[100] flex flex-col bg-black"
            onClick={(e) => {
              // Click on the backdrop (not on controls) closes too.
              if (e.target === e.currentTarget) setExpanded(false);
            }}
          >
            <video
              ref={expandedVideoRef}
              autoPlay
              playsInline
              disablePictureInPicture
              className="h-full w-full object-contain"
            />
            <div
              className="pointer-events-none absolute left-4 top-4 inline-flex items-center gap-2 rounded-full bg-black/55 px-3 py-1.5 text-xs font-medium text-white backdrop-blur"
            >
              <MonitorIcon className="h-3.5 w-3.5" />
              <span className="truncate">
                {tile.isLocal
                  ? `${tile.ownerDisplayName} (you)`
                  : tile.ownerDisplayName}
              </span>
            </div>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                setExpanded(false);
              }}
              className="absolute right-4 top-4 rounded-full bg-black/55 p-2 text-white transition-colors hover:bg-black/80"
              aria-label="Close"
            >
              <XIcon className="h-4 w-4" />
            </button>
            <div className="pointer-events-none absolute bottom-4 left-1/2 -translate-x-1/2 rounded-full bg-black/55 px-3 py-1.5 text-[11px] text-white/80 backdrop-blur">
              Esc to exit
            </div>
          </div>,
          document.body,
        )}

      {/* Owner glass pill */}
      <div
        className={cn(
          "pointer-events-none absolute left-0 right-0 flex items-center gap-1.5",
          PAD_BY_SIZE[size],
        )}
        style={{ top: "auto", bottom: 0 }}
      >
        <div
          className={cn(
            "inline-flex items-center gap-1.5 rounded-full px-2.5 py-[4px] font-medium text-white backdrop-blur",
            NAME_FONT_BY_SIZE[size],
          )}
          style={{ background: "rgba(20,22,30,0.55)" }}
        >
          <MonitorIcon className="h-3 w-3 text-emerald-400" />
          <span className="truncate">
            {tile.isLocal
              ? `${tile.ownerDisplayName}'s screen`
              : `${tile.ownerDisplayName}'s screen`}
          </span>
        </div>
      </div>
    </div>
  );
}

// ── Dispatcher ─────────────────────────────────────────────────────────────

export function RenderVideoTile({
  tile,
  size,
  onClick,
}: {
  tile: Tile;
  size: TileSize;
  onClick?: () => void;
}) {
  if (tile.kind === "screen")
    return <ScreenShareVideoTile tile={tile} size={size} onClick={onClick} />;
  return <CameraVideoTile tile={tile} size={size} onClick={onClick} />;
}
