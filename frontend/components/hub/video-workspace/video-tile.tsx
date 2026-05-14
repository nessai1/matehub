import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  HeadphoneOffIcon,
  MicOffIcon,
  MonitorIcon,
  Maximize2Icon,
  MoreVerticalIcon,
  Volume2Icon,
  VolumeXIcon,
  XIcon,
} from "lucide-react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Slider } from "@/components/ui/slider";
import { useVideoCall } from "@/contexts/video-call-context";
import { useParticipantVolume } from "@/hooks/use-participant-volume";
import { useTileAudio } from "@/hooks/use-tile-audio";
import { cn } from "@/lib/utils";
import { t } from "@/i18n";

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
  /** Distinct from `isMicMuted`: this user has self-deafened, meaning
   *  they can't hear ANYONE in the call. Rendered as a headphone-off
   *  badge so other participants know not to expect a verbal response.
   *  Broadcast through the SFU's `participant_deafened` event. */
  isDeafened: boolean;
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
  // Audio side-effects (deafen + per-participant volume, MAT-14/MAT-15)
  // flow in through context rather than props: the tile is composed in
  // half a dozen places (grid, spotlight, page reorders) and prop-
  // drilling two cross-cutting concerns through all of them is more
  // noise than the coupling is worth.
  //
  // The volume is read via useParticipantVolume (review #4): the hook
  // subscribes only to this user's key in the external store, so a
  // slider drag on another tile doesn't re-render this one.
  const { isDeafened, participantVolumeStore } = useVideoCall();
  const perTileVolume = useParticipantVolume(participantVolumeStore, tile.userId);

  // Web Audio pipeline: source → GainNode → destination. The <audio>
  // element behind audioRef stays muted and only exists so Safari keeps
  // the underlying track "playing"; output flows through Web Audio so
  // the slider can amplify past 100%.
  const audioRef = useTileAudio({
    track: tile.isLocal ? null : tile.audioTrack,
    volume: perTileVolume,
    muted: isDeafened,
  });

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

      {/* Per-participant volume menu (MAT-14). Local tile doesn't get one —
          there's nothing meaningful to attenuate on yourself (the local
          <video> is muted, mic level is a different control). */}
      {!tile.isLocal && size !== "xs" && (
        <TileVolumeMenu
          userId={tile.userId}
          displayName={tile.displayName}
          volume={perTileVolume}
        />
      )}

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
          {/* Mic-off — but suppressed if the user is also deafened.
              Deafen implies mic-off as a Discord-style coupling (you
              don't send when you can't hear), so showing both icons
              would be redundant noise. The headphone-off icon below
              carries the same "not participating right now" signal
              more clearly. */}
          {!tile.ringing && tile.isMicMuted && !tile.isDeafened && (
            <MicOffIcon
              className="h-3 w-3 text-[oklch(0.72_0.18_25)]"
              aria-label={t("Microphone muted")}
            />
          )}
          {/* Deafened — they can't hear ANYONE in the call (their own
              local mute of all incoming audio, broadcast via the SFU's
              participant_deafened event). Replaces the mic-off icon
              when set, since deafen implies mute. */}
          {!tile.ringing && tile.isDeafened && (
            <HeadphoneOffIcon
              className="h-3 w-3 text-[oklch(0.65_0.22_25)]"
              aria-label={t("Deafened")}
            />
          )}
          {/* Locally-muted-by-me — I dragged the volume slider on this
              participant to 0. Only meaningful for remote tiles (you
              can't mute yourself this way) and only when it's actually
              at 0. Distinct from isMicMuted: that's THEIR state; this
              is MY action on them. */}
          {!tile.ringing && !tile.isLocal && perTileVolume === 0 && (
            <VolumeXIcon
              className="h-3 w-3 text-zinc-400"
              aria-label={t("Volume muted for you")}
            />
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

  // Same deafen / volume coupling as CameraVideoTile, keyed by the
  // screen owner's user id so a single slider drives both their camera
  // and screen-share audio (typically just game audio).
  const { isDeafened, participantVolumeStore } = useVideoCall();
  const perTileVolume = useParticipantVolume(
    participantVolumeStore,
    tile.ownerUserId,
  );

  const audioRef = useTileAudio({
    track: tile.isLocal ? null : tile.audioTrack,
    volume: perTileVolume,
    muted: isDeafened,
  });

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

      {/* Volume menu for the screen audio — only when we actually have
          a remote audio track to control. Local screen-share self-
          preview has no audio (would echo into the call), so the menu
          would be a dead control. Positioned to the left of the
          Maximize button at `right-12` so they don't overlap. */}
      {tile.audioTrack && !tile.isLocal && (
        <TileVolumeMenu
          userId={tile.ownerUserId}
          displayName={tile.ownerDisplayName}
          volume={perTileVolume}
          triggerClassName="right-12 top-2"
        />
      )}

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

// ── Per-tile volume menu (MAT-14) ─────────────────────────────────────────
//
// The 3-dots affordance lives in the top-right corner of remote camera
// tiles. Right now it only carries a volume slider — future actions
// (kick from voice, global mute, etc.) drop into this same menu so the
// admin doesn't have to learn a second affordance.
//
// Why not a context-menu (right-click)? Phone/tablet users have no
// right-click equivalent. Click-to-open dropdown works everywhere.

function TileVolumeMenu({
  userId,
  displayName,
  volume,
  triggerClassName,
}: {
  userId: string;
  displayName: string;
  volume: number;
  /** Overrides the trigger button's positioning so the menu fits next
   *  to whatever other overlay controls a particular tile has (camera
   *  tile has nothing else; screen tile has a Maximize button at top-
   *  right, so we offset). Defaults to `right-2 top-2`. */
  triggerClassName?: string;
}) {
  const { setParticipantVolume } = useVideoCall();
  const [open, setOpen] = useState(false);
  // Remember the last non-zero volume so the Mute toggle can restore it
  // on un-mute. Without this, hitting Mute then Mute again would jump
  // from 0 → 100 even if the user was sitting at 130% before muting.
  // Local to the menu — closing the popover doesn't reset.
  const [lastUnmutedVolume, setLastUnmutedVolume] = useState<number>(
    volume > 0 ? volume : 1,
  );
  // Keep the remembered value in sync whenever the slider lands on a
  // non-zero level — covers the case where the user adjusts via slider
  // (not mute) and then hits Mute. setState-inside-effect normally
  // trips the cascading-renders lint, but here it's exactly the right
  // shape: an external-derived shadow value we update only when the
  // source crosses a threshold (>0). Splitting into a useRef + manual
  // assignment would dodge the lint at the cost of also dodging
  // React's batching — not worth it for a sub-byte of UI state.
  useEffect(() => {
    if (volume > 0) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setLastUnmutedVolume(volume);
    }
  }, [volume]);

  // Slider works in 0–200 to expose the [0, 2.0] range we now support
  // (HTMLMediaElement.volume caps at 1.0; the Web Audio GainNode handles
  // anything above — see hooks/use-tile-audio.ts).
  const sliderValue = Math.round(volume * 100);
  const isMuted = volume === 0;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        {/* stopPropagation: the whole tile is a click-to-spotlight target;
            opening the menu shouldn't also flip the spotlight. */}
        <button
          type="button"
          onClick={(e) => e.stopPropagation()}
          aria-label={t("Audio options for %s", displayName)}
          className={cn(
            "absolute grid h-7 w-7 place-items-center rounded-full text-white opacity-0 transition-opacity hover:bg-black/80",
            // Both group-hover modifiers so the trigger reveals on hover
            // of either parent — camera tile uses `group/tile`, screen
            // tile uses `group/screen`. Tailwind compiles both static
            // classes and the runtime picks whichever group matched.
            "group-hover/tile:opacity-100 group-hover/screen:opacity-100",
            "data-[state=open]:opacity-100",
            triggerClassName ?? "right-2 top-2",
          )}
          style={{ background: "rgba(20,22,30,0.55)" }}
          data-state={open ? "open" : "closed"}
        >
          <MoreVerticalIcon className="h-3.5 w-3.5" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        side="bottom"
        align="end"
        sideOffset={6}
        className="w-64 p-3"
        onClick={(e: React.MouseEvent) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2">
          {isMuted ? (
            <VolumeXIcon className="h-3.5 w-3.5 text-red-400" />
          ) : (
            <Volume2Icon className="h-3.5 w-3.5 text-muted-foreground" />
          )}
          <span className="text-xs font-medium">{t("Volume")}</span>
          <span
            className={cn(
              "ml-auto font-mono text-[10px]",
              isMuted ? "text-red-400" : "text-muted-foreground",
            )}
          >
            {sliderValue}%
          </span>
        </div>
        <Slider
          className="mt-2"
          min={0}
          max={200}
          step={1}
          value={[sliderValue]}
          onValueChange={(v: number[]) => {
            const next = v[0] ?? 100;
            setParticipantVolume(userId, next / 100);
          }}
        />
        <button
          type="button"
          onClick={() => {
            if (isMuted) {
              // Restore last non-zero level. If somehow we never had
              // one (initial mute via slider drag-to-zero) the
              // useState initialiser left it at 1.0 — sensible default.
              setParticipantVolume(userId, lastUnmutedVolume);
            } else {
              setParticipantVolume(userId, 0);
            }
          }}
          className={cn(
            "mt-3 flex w-full items-center justify-center gap-2 rounded-md border px-2 py-1.5 text-xs font-medium transition-colors",
            isMuted
              ? "border-red-900/30 bg-red-950/30 text-red-300 hover:bg-red-900/40"
              : "border-border hover:bg-accent hover:text-accent-foreground",
          )}
        >
          {isMuted ? (
            <>
              <Volume2Icon className="h-3.5 w-3.5" />
              {t("Unmute")}
            </>
          ) : (
            <>
              <VolumeXIcon className="h-3.5 w-3.5" />
              {t("Mute")}
            </>
          )}
        </button>
      </PopoverContent>
    </Popover>
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
