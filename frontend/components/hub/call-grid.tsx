"use client";

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import {
  ChevronLeftIcon,
  ChevronRightIcon,
  Maximize2Icon,
  MicOffIcon,
  MonitorIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { Participant } from "../../../packages/sdk-video/src";

export interface MemberInfo {
  displayName: string;
  avatarUrl?: string | null;
}

interface CallGridProps {
  participants: Participant[];
  localStream: MediaStream | null;
  /** Own screen-share preview. SFU doesn't loop the publisher's stream back,
   *  so the self-tile reads the local track directly. */
  localScreenVideoTrack: MediaStreamTrack | null;
  currentUserId: string;
  isCamEnabled: boolean;
  isMicEnabled: boolean;
  memberInfo: Record<string, MemberInfo>;
}

// ─── Tile shape ───────────────────────────────────────────────────────────

interface CameraTile {
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
}

interface ScreenTile {
  kind: "screen";
  id: string;
  ownerUserId: string;
  ownerDisplayName: string;
  videoTrack: MediaStreamTrack;
  audioTrack: MediaStreamTrack | null;
  isLocal: boolean;
}

type Tile = CameraTile | ScreenTile;

// ─── CallGrid ──────────────────────────────────────────────────────────────

export function CallGrid({
  participants,
  localStream,
  localScreenVideoTrack,
  currentUserId,
  isCamEnabled,
  memberInfo,
  isMicEnabled,
}: CallGridProps) {
  const localInfo = memberInfo[currentUserId];
  const localVideoTrack = isCamEnabled
    ? localStream?.getVideoTracks()[0] ?? null
    : null;

  const tiles: Tile[] = [];

  // Local camera tile (always present).
  tiles.push({
    kind: "camera",
    id: "local",
    userId: currentUserId,
    displayName: localInfo?.displayName ?? currentUserId,
    avatarUrl: localInfo?.avatarUrl ?? null,
    audioTrack: null, // never loopback our own mic
    videoTrack: localVideoTrack,
    isMicMuted: !isMicEnabled,
    isSpeaking: false,
    isLocal: true,
  });

  // Local screen-share preview — SFU doesn't loop it back, so we source the
  // local track directly.
  if (localScreenVideoTrack) {
    tiles.push({
      kind: "screen",
      id: "local-screen",
      ownerUserId: currentUserId,
      ownerDisplayName: `${localInfo?.displayName ?? currentUserId} (You)`,
      videoTrack: localScreenVideoTrack,
      audioTrack: null,
      isLocal: true,
    });
  }

  // Remote participants' camera + screen tracks.
  for (const p of participants) {
    const info = memberInfo[p.userId];
    tiles.push({
      kind: "camera",
      id: p.participantId,
      userId: p.userId,
      displayName: info?.displayName ?? p.userId,
      avatarUrl: info?.avatarUrl ?? null,
      audioTrack: p.audioTrack,
      videoTrack: p.videoTrack,
      isMicMuted: p.isMicMuted,
      isSpeaking: p.isSpeaking,
      isLocal: false,
    });
    if (p.screenVideoTrack) {
      tiles.push({
        kind: "screen",
        id: `${p.participantId}-screen`,
        ownerUserId: p.userId,
        ownerDisplayName: info?.displayName ?? p.userId,
        videoTrack: p.screenVideoTrack,
        audioTrack: p.screenAudioTrack,
        isLocal: false,
      });
    }
  }

  const [focusedTileId, setFocusedTileId] = useState<string | null>(null);

  // Derived, not stored: if the focused tile disappears (participant left,
  // screen-share stopped), we just treat focus as unset and render the grid.
  // The state itself stays so that if the tile comes back (flaky reconnect),
  // focus is restored automatically.
  const focusedTile = focusedTileId
    ? tiles.find((t) => t.id === focusedTileId) ?? null
    : null;

  // ─── Focus mode: big on top, horizontal strip below ────────────────────

  if (focusedTile) {
    const stripTiles = tiles.filter((t) => t.id !== focusedTile.id);
    return (
      <div className="flex h-full flex-col gap-2 p-3">
        <div className="flex min-h-0 flex-1">
          <RenderTile
            tile={focusedTile}
            variant="focused"
            onToggleFocus={() => setFocusedTileId(null)}
          />
        </div>
        {stripTiles.length > 0 && (
          <ScrollStrip>
            {stripTiles.map((t) => (
              <div
                key={t.id}
                className="aspect-video h-28 shrink-0 overflow-hidden rounded-lg"
              >
                <RenderTile
                  tile={t}
                  variant="strip"
                  onToggleFocus={() => setFocusedTileId(t.id)}
                />
              </div>
            ))}
          </ScrollStrip>
        )}
      </div>
    );
  }

  // ─── Grid mode: all tiles same size ────────────────────────────────────

  const gridCols =
    tiles.length <= 1
      ? "grid-cols-1"
      : tiles.length <= 4
        ? "grid-cols-2"
        : tiles.length <= 9
          ? "grid-cols-3"
          : "grid-cols-4";

  return (
    <div className={cn("grid gap-2 p-3", gridCols)}>
      {tiles.map((t) => (
        <RenderTile
          key={t.id}
          tile={t}
          variant="grid"
          onToggleFocus={() => setFocusedTileId(t.id)}
        />
      ))}
    </div>
  );
}

// ─── Tile renderer (dispatches by kind) ────────────────────────────────────

type TileVariant = "grid" | "focused" | "strip";

function RenderTile({
  tile,
  variant,
  onToggleFocus,
}: {
  tile: Tile;
  variant: TileVariant;
  onToggleFocus: () => void;
}) {
  if (tile.kind === "screen") {
    return (
      <ScreenShareTile
        tile={tile}
        variant={variant}
        onToggleFocus={onToggleFocus}
      />
    );
  }
  return (
    <ParticipantTile
      tile={tile}
      variant={variant}
      onToggleFocus={onToggleFocus}
    />
  );
}

// ─── Screen-share tile ─────────────────────────────────────────────────────

function ScreenShareTile({
  tile,
  variant,
  onToggleFocus,
}: {
  tile: ScreenTile;
  variant: TileVariant;
  onToggleFocus: () => void;
}) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const audioRef = useRef<HTMLAudioElement>(null);

  useEffect(() => {
    const el = videoRef.current;
    if (!el) return;
    el.srcObject = new MediaStream([tile.videoTrack]);
    el.play().catch(() => {});
  }, [tile.videoTrack]);

  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    if (tile.audioTrack) {
      el.srcObject = new MediaStream([tile.audioTrack]);
      el.play().catch(() => {});
    } else {
      el.srcObject = null;
    }
  }, [tile.audioTrack]);

  const fullscreen = useCallback(() => {
    const el = videoRef.current;
    if (!el) return;
    // requestFullscreen returns a promise; rejection (e.g. user-gesture
    // requirement, iframe permissions) is non-fatal.
    el.requestFullscreen().catch(() => {});
  }, []);

  return (
    <div
      className={cn(
        "group/screen relative flex min-h-0 w-full items-center justify-center overflow-hidden rounded-xl bg-black",
        variant === "focused" && "h-full flex-1",
        variant === "grid" && "aspect-video",
        variant === "strip" && "h-full w-full",
      )}
    >
      <video
        ref={videoRef}
        autoPlay
        playsInline
        onClick={onToggleFocus}
        className="h-full w-full cursor-pointer object-contain"
      />
      {tile.audioTrack && !tile.isLocal && (
        <audio ref={audioRef} autoPlay playsInline hidden />
      )}

      {/* Fullscreen button (only meaningful for screen-share). */}
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          fullscreen();
        }}
        className="absolute right-2 top-2 rounded-md bg-black/60 p-1.5 text-white opacity-0 transition-opacity hover:bg-black/80 group-hover/screen:opacity-100"
        aria-label="Fullscreen"
      >
        <Maximize2Icon className="h-3.5 w-3.5" />
      </button>

      {/* Owner label. */}
      <div className="absolute bottom-2 left-2 flex items-center gap-1.5 rounded-md bg-black/60 px-2 py-0.5">
        <MonitorIcon className="h-3 w-3 text-emerald-400" />
        <span className="text-xs font-medium text-white">
          {tile.ownerDisplayName}&apos;s screen
        </span>
      </div>
    </div>
  );
}

// ─── Camera tile ───────────────────────────────────────────────────────────

function ParticipantTile({
  tile,
  variant,
  onToggleFocus,
}: {
  tile: CameraTile;
  variant: TileVariant;
  onToggleFocus: () => void;
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

  return (
    <div
      onClick={onToggleFocus}
      className={cn(
        "relative flex cursor-pointer items-center justify-center overflow-hidden rounded-xl bg-muted/50 transition-all duration-200",
        tile.isSpeaking &&
          "ring-2 ring-emerald-500 ring-offset-2 ring-offset-background",
        variant === "focused" && "h-full w-full flex-1",
        variant === "grid" && "aspect-video",
        variant === "strip" && "h-full w-full",
      )}
    >
      {hasVideo ? (
        <video
          ref={videoRef}
          autoPlay
          playsInline
          muted={tile.isLocal}
          className={cn(
            "h-full w-full object-cover will-change-transform",
            tile.isLocal && "-scale-x-100",
          )}
        />
      ) : (
        <Avatar className="h-16 w-16">
          {tile.avatarUrl && <AvatarImage src={tile.avatarUrl} />}
          <AvatarFallback className="bg-primary/15 text-2xl font-semibold text-primary">
            {initials}
          </AvatarFallback>
        </Avatar>
      )}

      {!tile.isLocal && <audio ref={audioRef} autoPlay playsInline hidden />}

      <div className="absolute bottom-2 left-2 flex items-center gap-1.5 rounded-md bg-black/60 px-2 py-0.5">
        {tile.isMicMuted && <MicOffIcon className="h-3 w-3 text-red-400" />}
        <span className="text-xs font-medium text-white">
          {tile.isLocal ? `${tile.displayName} (You)` : tile.displayName}
        </span>
      </div>
    </div>
  );
}

// ─── Horizontal scroll strip with arrow "ears" ─────────────────────────────

function ScrollStrip({ children }: { children: React.ReactNode }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [canLeft, setCanLeft] = useState(false);
  const [canRight, setCanRight] = useState(false);

  const update = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const atStart = el.scrollLeft <= 1;
    const atEnd =
      Math.ceil(el.scrollLeft + el.clientWidth) >= el.scrollWidth - 1;
    setCanLeft(!atStart && el.scrollWidth > el.clientWidth);
    setCanRight(!atEnd && el.scrollWidth > el.clientWidth);
  }, []);

  // Arrow visibility is derived from DOM scroll geometry, which isn't reactive —
  // we have to sample it after layout and on resize. That inherently means
  // setState-inside-effect; the lint rule flags the whole hook body but the
  // tradeoff is accepted here.
  useLayoutEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    update();
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, [update, children]);

  const scrollBy = (delta: number) => {
    scrollRef.current?.scrollBy({ left: delta, behavior: "smooth" });
  };

  return (
    <div className="relative shrink-0">
      {canLeft && (
        <button
          type="button"
          onClick={() => scrollBy(-240)}
          aria-label="Scroll left"
          className="absolute left-0 top-1/2 z-10 flex -translate-y-1/2 items-center justify-center rounded-r-md bg-black/70 p-1 text-white hover:bg-black/90"
        >
          <ChevronLeftIcon className="h-4 w-4" />
        </button>
      )}
      <div
        ref={scrollRef}
        onScroll={update}
        className="flex snap-x gap-2 overflow-x-auto scroll-smooth [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      >
        {children}
      </div>
      {canRight && (
        <button
          type="button"
          onClick={() => scrollBy(240)}
          aria-label="Scroll right"
          className="absolute right-0 top-1/2 z-10 flex -translate-y-1/2 items-center justify-center rounded-l-md bg-black/70 p-1 text-white hover:bg-black/90"
        >
          <ChevronRightIcon className="h-4 w-4" />
        </button>
      )}
    </div>
  );
}
