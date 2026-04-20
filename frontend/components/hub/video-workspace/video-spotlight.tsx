"use client";

import { RenderVideoTile, type Tile } from "./video-tile";

export function VideoSpotlight({
  tiles,
  spotlightId,
  onPinTile,
}: {
  tiles: Tile[];
  spotlightId: string | null;
  onPinTile: (tileId: string) => void;
}) {
  if (tiles.length === 0) return null;

  const big = tiles.find((t) => t.id === spotlightId) ?? tiles[0];
  const bigWithPin: Tile =
    big.kind === "camera" ? { ...big, pinned: true } : { ...big, pinned: true };
  const strip = tiles.filter((t) => t.id !== big.id);

  return (
    <div
      className="grid h-full w-full gap-2.5"
      style={{
        gridTemplateRows: "1fr auto",
        padding: 18,
        paddingBottom: 86,
      }}
    >
      {/* Big tile */}
      <div className="flex h-full min-h-0 w-full items-center justify-center">
        <RenderVideoTile
          tile={bigWithPin}
          size="lg"
          onClick={() => onPinTile(bigWithPin.id)}
        />
      </div>

      {/* Strip */}
      {strip.length > 0 && (
        <div
          className="grid gap-2"
          style={{
            height: 92,
            gridTemplateColumns: `repeat(${Math.min(strip.length, 8)}, minmax(0, 1fr))`,
          }}
        >
          {strip.slice(0, 8).map((t) => (
            <div
              key={t.id}
              className="flex min-h-0 min-w-0 items-center justify-center"
            >
              <RenderVideoTile
                tile={t}
                size="xs"
                onClick={() => onPinTile(t.id)}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
