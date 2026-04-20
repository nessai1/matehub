import { useEffect, useRef, useState } from "react";
import { RenderVideoTile, type Tile, type TileSize } from "./video-tile";

export const PAGE_SIZE = 12;

const TILE_ASPECT = 16 / 10;
const GAP = 10;

function pickTileSize(tilePx: number): TileSize {
  if (tilePx < 110) return "xs";
  if (tilePx < 180) return "sm";
  if (tilePx < 260) return "md";
  return "lg";
}

/**
 * Maximises 16:10 tile area in the available container by brute-forcing
 * `cols` ∈ [1, count]. Classic video-call grid math — same approach Zoom/Meet
 * use for "Gallery view".
 */
function computeLayout(
  count: number,
  containerW: number,
  containerH: number,
): { cols: number; rows: number; tileW: number; tileH: number } {
  if (count === 0 || containerW <= 0 || containerH <= 0)
    return { cols: 1, rows: 1, tileW: 0, tileH: 0 };

  let best = { area: 0, cols: 1, rows: 1, tileW: 0, tileH: 0 };
  for (let cols = 1; cols <= count; cols++) {
    const rows = Math.ceil(count / cols);
    const cellW = (containerW - GAP * (cols - 1)) / cols;
    const cellH = (containerH - GAP * (rows - 1)) / rows;
    if (cellW <= 0 || cellH <= 0) continue;

    // Fit a 16:10 rectangle inside cellW × cellH.
    let tileW = cellW;
    let tileH = cellW / TILE_ASPECT;
    if (tileH > cellH) {
      tileH = cellH;
      tileW = cellH * TILE_ASPECT;
    }
    const area = tileW * tileH;
    if (area > best.area) best = { area, cols, rows, tileW, tileH };
  }
  return best;
}

function useContainerSize(): {
  ref: React.RefObject<HTMLDivElement | null>;
  w: number;
  h: number;
} {
  const ref = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      setSize({ w: r.width, h: r.height });
    });
    ro.observe(el);
    // Also sample once synchronously so the first paint has correct numbers.
    const r = el.getBoundingClientRect();
    setSize({ w: r.width, h: r.height });
    return () => ro.disconnect();
  }, []);

  return { ref, w: size.w, h: size.h };
}

export function VideoGrid({
  tiles,
  onPinTile,
}: {
  tiles: Tile[];
  onPinTile: (tileId: string) => void;
}) {
  const { ref, w, h } = useContainerSize();
  const { cols, rows, tileW, tileH } = computeLayout(tiles.length, w, h);
  const tileSize = pickTileSize(tileH);

  return (
    <div
      className="absolute inset-0"
      style={{
        padding: 18,
        paddingBottom: 86, // leave room for the floating controls
      }}
    >
      <div
        ref={ref}
        className="flex h-full w-full items-center justify-center"
      >
        {tileW > 0 && tileH > 0 && (
          <div
            style={{
              display: "grid",
              gridTemplateColumns: `repeat(${cols}, ${tileW}px)`,
              gridTemplateRows: `repeat(${rows}, ${tileH}px)`,
              gap: GAP,
            }}
          >
            {tiles.map((t) => (
              <RenderVideoTile
                key={t.id}
                tile={t}
                size={tileSize}
                onClick={() => onPinTile(t.id)}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
