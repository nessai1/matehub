"use client";

import { useCallback, useEffect, useState } from "react";
import { XIcon, ChevronLeftIcon, ChevronRightIcon } from "lucide-react";
import { cn } from "@/lib/utils";

interface Props {
  /** All images in the gallery */
  images: { url: string; alt?: string }[];
  /** Initial index to show */
  initialIndex?: number;
  onClose: () => void;
}

export function ImageLightbox({ images, initialIndex = 0, onClose }: Props) {
  const [index, setIndex] = useState(initialIndex);
  const [visible, setVisible] = useState(false);
  const [closing, setClosing] = useState(false);

  useEffect(() => {
    const t = requestAnimationFrame(() => setVisible(true));
    return () => cancelAnimationFrame(t);
  }, []);

  const handleClose = useCallback(() => {
    setClosing(true);
    setTimeout(onClose, 200);
  }, [onClose]);

  const prev = useCallback(() => {
    setIndex((i) => (i - 1 + images.length) % images.length);
  }, [images.length]);

  const next = useCallback(() => {
    setIndex((i) => (i + 1) % images.length);
  }, [images.length]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") handleClose();
      else if (e.key === "ArrowLeft") prev();
      else if (e.key === "ArrowRight") next();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [handleClose, prev, next]);

  const current = images[index];
  if (!current) return null;
  const hasMultiple = images.length > 1;

  return (
    <div
      className={cn(
        "fixed inset-0 z-50 flex items-center justify-center p-4 transition-all duration-200",
        visible && !closing ? "bg-black/90 backdrop-blur-sm" : "bg-black/0 backdrop-blur-none",
      )}
      onClick={handleClose}
    >
      {/* Close button */}
      <button
        onClick={handleClose}
        className={cn(
          "absolute top-4 right-4 z-10 flex h-10 w-10 items-center justify-center rounded-full bg-white/10 text-white backdrop-blur transition-all duration-200 hover:bg-white/20",
          visible && !closing ? "opacity-100" : "opacity-0",
        )}
      >
        <XIcon className="h-5 w-5" />
      </button>

      {/* Counter */}
      {hasMultiple && (
        <div
          className={cn(
            "absolute top-4 left-1/2 z-10 -translate-x-1/2 rounded-full bg-white/10 px-3 py-1.5 text-xs text-white backdrop-blur transition-opacity duration-200",
            visible && !closing ? "opacity-100" : "opacity-0",
          )}
        >
          {index + 1} / {images.length}
        </div>
      )}

      {/* Prev */}
      {hasMultiple && (
        <button
          onClick={(e) => { e.stopPropagation(); prev(); }}
          className={cn(
            "absolute left-4 top-1/2 z-10 flex h-12 w-12 -translate-y-1/2 items-center justify-center rounded-full bg-white/10 text-white backdrop-blur transition-all duration-200 hover:bg-white/20",
            visible && !closing ? "opacity-100" : "opacity-0",
          )}
        >
          <ChevronLeftIcon className="h-6 w-6" />
        </button>
      )}

      {/* Image */}
      <img
        key={current.url}
        src={current.url}
        alt={current.alt || ""}
        className={cn(
          "max-h-full max-w-full object-contain transition-all duration-300 ease-out",
          visible && !closing ? "scale-100 opacity-100" : "scale-90 opacity-0",
        )}
        onClick={(e) => e.stopPropagation()}
      />

      {/* Next */}
      {hasMultiple && (
        <button
          onClick={(e) => { e.stopPropagation(); next(); }}
          className={cn(
            "absolute right-4 top-1/2 z-10 flex h-12 w-12 -translate-y-1/2 items-center justify-center rounded-full bg-white/10 text-white backdrop-blur transition-all duration-200 hover:bg-white/20",
            visible && !closing ? "opacity-100" : "opacity-0",
          )}
        >
          <ChevronRightIcon className="h-6 w-6" />
        </button>
      )}
    </div>
  );
}
