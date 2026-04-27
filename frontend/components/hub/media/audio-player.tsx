import { useCallback, useEffect, useRef, useState } from "react";
import {
  PlayIcon,
  PauseIcon,
  Volume1Icon,
  Volume2Icon,
  VolumeXIcon,
  DownloadIcon,
  Music2Icon,
} from "lucide-react";
import { cn } from "@/lib/utils";

interface Props {
  url: string;
  name: string;
  size: number;
  duration?: number;
  className?: string;
}

const VOLUME_STORAGE_KEY = "matehub.audio.volume";
const MUTED_STORAGE_KEY = "matehub.audio.muted";

function readPersistedVolume(): number {
  if (typeof window === "undefined") return 1;
  const raw = window.localStorage.getItem(VOLUME_STORAGE_KEY);
  if (raw === null) return 1;
  const v = parseFloat(raw);
  if (!isFinite(v)) return 1;
  return Math.max(0, Math.min(1, v));
}

function readPersistedMuted(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(MUTED_STORAGE_KEY) === "1";
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function fmtTime(sec: number): string {
  if (!isFinite(sec) || sec < 0) return "0:00";
  const m = Math.floor(sec / 60);
  const s = Math.floor(sec % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/**
 * Custom HTML5 audio player.
 *
 * Browser handles HTTP Range requests automatically when <audio> loads a
 * remote URL — native seek support and lazy loading come for free.
 */
export function AudioPlayer({ url, name, size, duration: durationHint, className }: Props) {
  const audioRef = useRef<HTMLAudioElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const volBarRef = useRef<HTMLDivElement>(null);
  const [playing, setPlaying] = useState(false);
  const [muted, setMuted] = useState(false);
  const [volume, setVolume] = useState(1);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(durationHint ?? 0);
  const [buffered, setBuffered] = useState(0);
  const [scrubPct, setScrubPct] = useState<number | null>(null);

  // Restore persisted volume/muted on mount
  useEffect(() => {
    const v = readPersistedVolume();
    const m = readPersistedMuted();
    setVolume(v);
    setMuted(m);
    const a = audioRef.current;
    if (a) {
      a.volume = v;
      a.muted = m;
    }
  }, []);

  // Apply volume/muted to <audio> element
  useEffect(() => {
    const a = audioRef.current;
    if (!a) return;
    a.volume = volume;
    a.muted = muted;
  }, [volume, muted]);

  const togglePlay = useCallback(() => {
    const a = audioRef.current;
    if (!a) return;
    if (a.paused) a.play();
    else a.pause();
  }, []);

  const toggleMute = useCallback(() => {
    setMuted((prev) => {
      const next = !prev;
      window.localStorage.setItem(MUTED_STORAGE_KEY, next ? "1" : "0");
      // If we're unmuting from a zero volume, bump it to something audible
      if (!next && volume === 0) {
        setVolume(0.5);
        window.localStorage.setItem(VOLUME_STORAGE_KEY, "0.5");
      }
      return next;
    });
  }, [volume]);

  const applyVolume = useCallback((v: number) => {
    const clamped = Math.max(0, Math.min(1, v));
    setVolume(clamped);
    window.localStorage.setItem(VOLUME_STORAGE_KEY, clamped.toString());
    // Dragging volume implicitly unmutes (unless dragged to 0)
    if (clamped > 0 && muted) {
      setMuted(false);
      window.localStorage.setItem(MUTED_STORAGE_KEY, "0");
    }
  }, [muted]);

  const seekToPct = useCallback((pct: number) => {
    const a = audioRef.current;
    if (!a || !a.duration) return;
    a.currentTime = a.duration * Math.max(0, Math.min(1, pct));
  }, []);

  useEffect(() => {
    const a = audioRef.current;
    if (!a) return;

    const onTime = () => setCurrentTime(a.currentTime);
    const onDur = () => setDuration(a.duration || 0);
    const onPlay = () => setPlaying(true);
    const onPause = () => setPlaying(false);
    const onEnd = () => setPlaying(false);
    const onProgress = () => {
      if (!a.duration) return;
      let maxEnd = 0;
      for (let i = 0; i < a.buffered.length; i++) {
        const end = a.buffered.end(i);
        if (end > maxEnd) maxEnd = end;
      }
      setBuffered(maxEnd / a.duration);
    };

    a.addEventListener("timeupdate", onTime);
    a.addEventListener("durationchange", onDur);
    a.addEventListener("loadedmetadata", onDur);
    a.addEventListener("play", onPlay);
    a.addEventListener("pause", onPause);
    a.addEventListener("ended", onEnd);
    a.addEventListener("progress", onProgress);
    return () => {
      a.removeEventListener("timeupdate", onTime);
      a.removeEventListener("durationchange", onDur);
      a.removeEventListener("loadedmetadata", onDur);
      a.removeEventListener("play", onPlay);
      a.removeEventListener("pause", onPause);
      a.removeEventListener("ended", onEnd);
      a.removeEventListener("progress", onProgress);
    };
  }, []);

  // Generic drag-to-set helper: tracks a horizontal bar via PointerEvents,
  // calls onChange on every movement, onCommit on release.
  const dragHandler = useCallback(
    (
      bar: HTMLDivElement,
      e: React.PointerEvent<HTMLDivElement>,
      onChange: (pct: number) => void,
      onCommit?: (pct: number) => void,
    ) => {
      const rect = bar.getBoundingClientRect();
      const compute = (clientX: number) =>
        Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));

      const initial = compute(e.clientX);
      onChange(initial);
      bar.setPointerCapture(e.pointerId);

      const onMove = (ev: PointerEvent) => onChange(compute(ev.clientX));
      const onUp = (ev: PointerEvent) => {
        const final = compute(ev.clientX);
        onCommit?.(final);
        bar.releasePointerCapture(ev.pointerId);
        bar.removeEventListener("pointermove", onMove);
        bar.removeEventListener("pointerup", onUp);
        bar.removeEventListener("pointercancel", onUp);
      };
      bar.addEventListener("pointermove", onMove);
      bar.addEventListener("pointerup", onUp);
      bar.addEventListener("pointercancel", onUp);
    },
    [],
  );

  const onSeekBarPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const bar = barRef.current;
      if (!bar) return;
      dragHandler(
        bar,
        e,
        (pct) => setScrubPct(pct),
        (pct) => {
          seekToPct(pct);
          setScrubPct(null);
        },
      );
    },
    [dragHandler, seekToPct],
  );

  const onVolumeBarPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const bar = volBarRef.current;
      if (!bar) return;
      dragHandler(bar, e, applyVolume);
    },
    [dragHandler, applyVolume],
  );

  const playedPct = scrubPct !== null
    ? scrubPct
    : duration
    ? currentTime / duration
    : 0;
  const displayTime = scrubPct !== null && duration
    ? scrubPct * duration
    : currentTime;

  const effectivelyMuted = muted || volume === 0;
  const VolumeIcon = effectivelyMuted
    ? VolumeXIcon
    : volume < 0.5
    ? Volume1Icon
    : Volume2Icon;
  const visualVolume = effectivelyMuted ? 0 : volume;

  return (
    <div
      className={cn(
        "flex w-full max-w-md items-center gap-3 rounded-lg border border-border/50 bg-muted/30 px-3 py-2.5",
        className,
      )}
    >
      <audio ref={audioRef} src={url} preload="metadata" className="hidden" />

      <button
        onClick={togglePlay}
        aria-label={playing ? "Pause" : "Play"}
        className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground transition-opacity hover:opacity-90"
      >
        {playing ? (
          <PauseIcon className="h-4 w-4" fill="currentColor" />
        ) : (
          <PlayIcon className="h-4 w-4 translate-x-[1px]" fill="currentColor" />
        )}
      </button>

      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex items-center gap-2 text-xs">
          <Music2Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <span className="truncate font-medium" title={name}>
            {name}
          </span>
        </div>

        <div
          ref={barRef}
          onPointerDown={onSeekBarPointerDown}
          className="group relative h-1.5 cursor-pointer rounded-full bg-foreground/10"
        >
          <div
            className="absolute inset-y-0 left-0 rounded-full bg-foreground/20"
            style={{ width: `${buffered * 100}%` }}
          />
          <div
            className="absolute inset-y-0 left-0 rounded-full bg-primary"
            style={{ width: `${playedPct * 100}%` }}
          />
          <div
            className="absolute top-1/2 h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-full bg-primary opacity-0 shadow transition-opacity group-hover:opacity-100"
            style={{ left: `${playedPct * 100}%` }}
          />
        </div>

        <div className="flex items-center justify-between text-[10px] font-mono text-muted-foreground">
          <span>
            {fmtTime(displayTime)} / {fmtTime(duration)}
          </span>
          <span>{formatSize(size)}</span>
        </div>
      </div>

      <div className="flex shrink-0 items-center gap-1">
        {/* Volume control: hover the group to expand the slider. */}
        <div className="group/vol flex items-center">
          <div className="overflow-hidden transition-[width,opacity,margin] duration-150 w-0 opacity-0 group-hover/vol:w-16 group-hover/vol:opacity-100 group-hover/vol:mr-1">
            <div
              ref={volBarRef}
              onPointerDown={onVolumeBarPointerDown}
              role="slider"
              aria-label="Volume"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(visualVolume * 100)}
              className="relative h-1.5 w-16 cursor-pointer rounded-full bg-foreground/10"
            >
              <div
                className="absolute inset-y-0 left-0 rounded-full bg-foreground"
                style={{ width: `${visualVolume * 100}%` }}
              />
              <div
                className="absolute top-1/2 h-2.5 w-2.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-foreground shadow"
                style={{ left: `${visualVolume * 100}%` }}
              />
            </div>
          </div>
          <button
            onClick={toggleMute}
            aria-label={effectivelyMuted ? "Unmute" : "Mute"}
            className="flex h-7 w-7 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
          >
            <VolumeIcon className="h-4 w-4" />
          </button>
        </div>
        <a
          href={url}
          download={name}
          target="_blank"
          rel="noopener"
          aria-label="Download"
          className="flex h-7 w-7 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
        >
          <DownloadIcon className="h-4 w-4" />
        </a>
      </div>
    </div>
  );
}
