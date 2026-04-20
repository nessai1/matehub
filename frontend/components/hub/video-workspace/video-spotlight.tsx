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

      {/* Strip — fixed-height row of 16:10 thumbnails. Flex (not grid 1fr)
          so tiles keep their aspect ratio when there are just a few of them
          instead of stretching to fill the row width. */}
      {strip.length > 0 && (
        <div
          className="flex gap-2 overflow-x-auto"
          style={{ height: 92 }}
        >
          {strip.slice(0, 8).map((t) => (
            <div
              key={t.id}
              className="relative h-full shrink-0"
              style={{ aspectRatio: "16 / 10" }}
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
