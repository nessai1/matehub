import { useCallback, useEffect, useRef, useState } from "react";
import {
  PlayIcon,
  PauseIcon,
  Volume2Icon,
  VolumeXIcon,
  MaximizeIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";

interface Props {
  url: string;
  poster?: string;
  /** Optional hint for aspect ratio */
  width?: number;
  height?: number;
  className?: string;
}

/**
 * Custom HTML5 video player.
 *
 * Browser handles HTTP Range requests automatically when the <video> element
 * loads a remote URL -- this gives us native seek support and lazy loading
 * (only the visible/buffered portion is downloaded).
 *
 * Shows buffered progress bar on the seek slider (like YouTube).
 */
export function VideoPlayer({ url, poster, width, height, className }: Props) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [playing, setPlaying] = useState(false);
  const [muted, setMuted] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [buffered, setBuffered] = useState<number>(0);
  const [hovering, setHovering] = useState(false);

  const togglePlay = useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) v.play();
    else v.pause();
  }, []);

  const toggleMute = useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    v.muted = !v.muted;
    setMuted(v.muted);
  }, []);

  const fullscreen = useCallback(() => {
    videoRef.current?.requestFullscreen?.();
  }, []);

  const seek = useCallback((pct: number) => {
    const v = videoRef.current;
    if (!v || !v.duration) return;
    v.currentTime = v.duration * pct;
  }, []);

  // Track buffered ranges (for buffer bar)
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;

    const updateBuffered = () => {
      if (!v.duration) return;
      let maxEnd = 0;
      for (let i = 0; i < v.buffered.length; i++) {
        const end = v.buffered.end(i);
        if (end > maxEnd) maxEnd = end;
      }
      setBuffered(maxEnd / v.duration);
    };

    v.addEventListener("progress", updateBuffered);
    v.addEventListener("timeupdate", () => setCurrentTime(v.currentTime));
    v.addEventListener("durationchange", () => setDuration(v.duration || 0));
    v.addEventListener("play", () => setPlaying(true));
    v.addEventListener("pause", () => setPlaying(false));
    return () => {
      v.removeEventListener("progress", updateBuffered);
    };
  }, []);

  const fmtTime = (sec: number): string => {
    if (!isFinite(sec)) return "0:00";
    const m = Math.floor(sec / 60);
    const s = Math.floor(sec % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  const progress = duration ? currentTime / duration : 0;
  const aspect = width && height ? `${width}/${height}` : undefined;

  return (
    <div
      className={cn(
        "group relative overflow-hidden rounded-lg bg-black",
        className,
      )}
      style={aspect ? { aspectRatio: aspect } : undefined}
      onMouseEnter={() => setHovering(true)}
      onMouseLeave={() => setHovering(false)}
    >
      <video
        ref={videoRef}
        src={url}
        poster={poster}
        preload="metadata"
        playsInline
        className="h-full w-full cursor-pointer"
        onClick={togglePlay}
      />

      {/* Big play button overlay when paused */}
      {!playing && (
        <button
          onClick={togglePlay}
          className="absolute inset-0 flex items-center justify-center bg-black/20 transition-opacity"
        >
          <div className="flex h-16 w-16 items-center justify-center rounded-full bg-black/60 backdrop-blur">
            <PlayIcon className="h-8 w-8 text-white" fill="white" />
          </div>
        </button>
      )}

      {/* Controls bar */}
      <div
        className={cn(
          "absolute inset-x-0 bottom-0 flex flex-col gap-1 bg-gradient-to-t from-black/80 to-transparent px-3 py-2 transition-opacity",
          playing && !hovering ? "opacity-0" : "opacity-100",
        )}
      >
        {/* Progress bar */}
        <div
          className="group/bar relative h-1 cursor-pointer rounded-full bg-white/20"
          onClick={(e) => {
            const rect = e.currentTarget.getBoundingClientRect();
            seek((e.clientX - rect.left) / rect.width);
          }}
        >
          {/* Buffered */}
          <div
            className="absolute inset-y-0 left-0 rounded-full bg-white/40"
            style={{ width: `${buffered * 100}%` }}
          />
          {/* Played */}
          <div
            className="absolute inset-y-0 left-0 rounded-full bg-white"
            style={{ width: `${progress * 100}%` }}
          />
        </div>

        {/* Controls row */}
        <div className="flex items-center gap-2 text-white text-xs">
          <button onClick={togglePlay} className="hover:opacity-80">
            {playing ? (
              <PauseIcon className="h-4 w-4" />
            ) : (
              <PlayIcon className="h-4 w-4" />
            )}
          </button>
          <button onClick={toggleMute} className="hover:opacity-80">
            {muted ? (
              <VolumeXIcon className="h-4 w-4" />
            ) : (
              <Volume2Icon className="h-4 w-4" />
            )}
          </button>
          <span className="font-mono text-[10px]">
            {fmtTime(currentTime)} / {fmtTime(duration)}
          </span>
          <button
            onClick={fullscreen}
            className="ml-auto hover:opacity-80"
          >
            <MaximizeIcon className="h-4 w-4" />
          </button>
        </div>
      </div>
    </div>
  );
}
