import { useMemo, useState } from "react";
import { ChevronLeftIcon, ChevronRightIcon } from "lucide-react";
import { ChannelIcon, type Channel } from "@/components/nav-channels";
import { useVideoCall } from "@/contexts/video-call-context";
import { useMembers } from "@/hooks/use-members";
import { useAuth } from "@/lib/auth";
import { CallDebugPanel } from "@/components/hub/call-debug-panel";
import { ScreenShareProfileDialog } from "@/components/hub/screen-share-profile-dialog";
import { cn } from "@/lib/utils";
import { VideoGrid, PAGE_SIZE } from "./video-grid";
import { VideoSpotlight } from "./video-spotlight";
import { VideoCallControls } from "./video-controls";
import type { CameraTile, ScreenTile, Tile } from "./video-tile";

type LayoutMode = "grid" | "spotlight";

interface VideoWorkspaceProps {
  channel: Channel;
}

export function VideoWorkspace({ channel }: VideoWorkspaceProps) {
  const channelId = channel.id;
  const channelName = channel.name;
  const { session } = useAuth();
  const { members } = useMembers();
  const username = session?.username ?? "anonymous";
  const localUserId = session?.userId ?? "local";

  const {
    activeVoiceChannelId,
    participants,
    localStream,
    localScreenVideoTrack,
    isConnected,
    isMicEnabled,
    isCamEnabled,
    isScreenSharing,
    toggleMic,
    toggleCamera,
    publishScreen,
    unpublishScreen,
    leaveVoice,
    client,
  } = useVideoCall();

  const [layout, setLayout] = useState<LayoutMode>("grid");
  const [spotlightId, setSpotlightId] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [shareDialogOpen, setShareDialogOpen] = useState(false);

  // ── Member → displayName/avatar lookup ───────────────────────────────────
  // Keyed by Snowflake user_id — SDK participants carry user_id, not username.
  const memberLookup = useMemo(() => {
    const map: Record<
      string,
      { displayName: string; avatarUrl: string | null }
    > = {};
    if (session) {
      map[session.userId] = {
        displayName: session.displayName,
        avatarUrl: session.avatarUrl ?? null,
      };
    }
    for (const m of members) {
      map[m.user_id] = {
        displayName: m.display_name,
        avatarUrl: m.avatar_url ?? null,
      };
    }
    return map;
  }, [session, members]);

  const showingActive = activeVoiceChannelId === channelId;

  // ── Build tiles ──────────────────────────────────────────────────────────
  // Order:
  //   1. local camera (always first — self-anchor)
  //   2. remote cameras
  //   3. screen shares at the end (less common, flow-disturbing)
  // The speaker-priority re-sort (see below) then hoists active speakers.

  const tiles: Tile[] = useMemo(() => {
    const out: Tile[] = [];
    const localInfo = memberLookup[localUserId];
    const localVideoTrack = isCamEnabled
      ? localStream?.getVideoTracks()[0] ?? null
      : null;

    const localCamTile: CameraTile = {
      kind: "camera",
      id: "local",
      userId: localUserId,
      displayName: localInfo?.displayName ?? session?.displayName ?? username,
      avatarUrl: localInfo?.avatarUrl ?? session?.avatarUrl ?? null,
      audioTrack: null,
      videoTrack: localVideoTrack,
      isMicMuted: !isMicEnabled,
      isSpeaking: false, // we don't self-detect speaking
      isLocal: true,
    };
    out.push(localCamTile);

    for (const p of participants) {
      const info = memberLookup[p.userId];
      const t: CameraTile = {
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
      };
      out.push(t);
    }

    // Screen shares come after cameras — append at the tail.
    if (localScreenVideoTrack) {
      const info = memberLookup[localUserId];
      const s: ScreenTile = {
        kind: "screen",
        id: "local-screen",
        ownerUserId: localUserId,
        ownerDisplayName: info?.displayName ?? session?.displayName ?? username,
        videoTrack: localScreenVideoTrack,
        audioTrack: null,
        isLocal: true,
      };
      out.push(s);
    }
    for (const p of participants) {
      if (p.screenVideoTrack) {
        const info = memberLookup[p.userId];
        const s: ScreenTile = {
          kind: "screen",
          id: `${p.participantId}-screen`,
          ownerUserId: p.userId,
          ownerDisplayName: info?.displayName ?? p.userId,
          videoTrack: p.screenVideoTrack,
          audioTrack: p.screenAudioTrack,
          isLocal: false,
        };
        out.push(s);
      }
    }

    return out;
  }, [
    memberLookup,
    localUserId,
    username,
    session,
    participants,
    localStream,
    localScreenVideoTrack,
    isCamEnabled,
    isMicEnabled,
  ]);

  // Speaker-priority: active speakers hoist to the front so the first page
  // shows them even in a large call. Local tile still anchored first.
  const sortedTiles = useMemo(() => {
    if (tiles.length <= PAGE_SIZE) return tiles;
    const local = tiles.filter((t) => "isLocal" in t && t.isLocal);
    const remote = tiles.filter((t) => !("isLocal" in t) || !t.isLocal);
    remote.sort((a, b) => {
      const aSpeak = a.kind === "camera" ? (a.isSpeaking ? 1 : 0) : 0;
      const bSpeak = b.kind === "camera" ? (b.isSpeaking ? 1 : 0) : 0;
      return bSpeak - aSpeak;
    });
    return [...local, ...remote];
  }, [tiles]);

  // ── Pagination ───────────────────────────────────────────────────────────
  const pages = Math.max(1, Math.ceil(sortedTiles.length / PAGE_SIZE));
  // Clamp derived-ly so we don't need a sync-in-effect just to keep it in
  // range when participants come and go.
  const clampedPage = Math.min(page, pages - 1);

  const pageTiles =
    pages > 1
      ? sortedTiles.slice(clampedPage * PAGE_SIZE, (clampedPage + 1) * PAGE_SIZE)
      : sortedTiles;

  // ── Spotlight toggle: click a tile → pin it ──────────────────────────────
  const onPinTile = (tileId: string) => {
    if (layout === "spotlight" && spotlightId === tileId) {
      // Double-pin toggles back to grid.
      setLayout("grid");
      setSpotlightId(null);
      return;
    }
    setLayout("spotlight");
    setSpotlightId(tileId);
  };

  const totalInCall = tiles.filter((t) => t.kind === "camera").length;

  return (
    <div
      className="relative flex h-full min-h-0 flex-col overflow-hidden text-white"
      style={{
        background:
          "radial-gradient(ellipse at 30% 20%, oklch(0.24 0.03 270) 0%, oklch(0.16 0.01 260) 55%, oklch(0.12 0.008 260) 100%)",
      }}
    >
      {/* ── Header (44px, glass on dark) ─────────────────────────────────── */}
      <header
        className="relative z-10 flex h-11 shrink-0 items-center gap-3 px-4"
        style={{
          background: "rgba(10,12,20,0.35)",
          backdropFilter: "blur(8px)",
          borderBottom: "1px solid rgba(255,255,255,0.06)",
        }}
      >
        <ChannelIcon channel={channel} />
        <span className="text-sm font-semibold">{channelName}</span>

        <span
          className="inline-flex items-center gap-1.5 rounded px-[7px] py-[2px] font-mono text-[11px] text-white/70"
          style={{ background: "rgba(255,255,255,0.08)" }}
        >
          <span
            className="h-[6px] w-[6px] rounded-full"
            style={{
              background: "oklch(0.72 0.18 150)",
              boxShadow: "0 0 0 2px oklch(0.72 0.18 150 / 0.25)",
            }}
          />
          {showingActive && isConnected
            ? "Connected"
            : showingActive
              ? "Connecting…"
              : "Not in call"}
        </span>

        <div className="flex-1" />

        {pages > 1 && (
          <div
            className="inline-flex items-center gap-1.5 rounded-full py-1 pl-2.5 pr-1 font-mono text-[11px] text-white"
            style={{ background: "rgba(255,255,255,0.08)" }}
          >
            <span className="text-white/55">page</span>
            <span className="font-semibold">
              {clampedPage + 1} / {pages}
            </span>
            <div className="ml-1 inline-flex gap-[2px]">
              <button
                type="button"
                disabled={clampedPage === 0}
                onClick={() => setPage(Math.max(0, clampedPage - 1))}
                className={cn(
                  "grid h-5 w-5 place-items-center rounded-full transition-colors",
                  clampedPage === 0
                    ? "cursor-not-allowed opacity-40"
                    : "hover:bg-white/10",
                )}
                style={{ background: "rgba(255,255,255,0.1)" }}
                aria-label="Previous page"
              >
                <ChevronLeftIcon className="h-3 w-3" />
              </button>
              <button
                type="button"
                disabled={clampedPage + 1 >= pages}
                onClick={() => setPage(Math.min(pages - 1, clampedPage + 1))}
                className={cn(
                  "grid h-5 w-5 place-items-center rounded-full transition-colors",
                  clampedPage + 1 >= pages
                    ? "cursor-not-allowed opacity-40"
                    : "hover:bg-white/10",
                )}
                style={{ background: "rgba(255,255,255,0.1)" }}
                aria-label="Next page"
              >
                <ChevronRightIcon className="h-3 w-3" />
              </button>
            </div>
          </div>
        )}

        <span
          className="inline-flex items-center gap-1.5 rounded px-[9px] py-[3px] font-mono text-[11px] text-white/75"
          style={{ background: "rgba(255,255,255,0.08)" }}
        >
          <span className="text-white/50">●</span> {totalInCall} in call
        </span>

        {/* Layout chooser (grid / spot) */}
        <div
          className="inline-flex gap-0.5 rounded-lg p-0.5"
          style={{ background: "rgba(255,255,255,0.06)" }}
        >
          {(
            [
              { id: "grid", label: "grid", icon: "▦" },
              { id: "spotlight", label: "spot", icon: "▣" },
            ] as const
          ).map((opt) => {
            const on =
              (opt.id === "grid" && layout === "grid") ||
              (opt.id === "spotlight" && layout === "spotlight");
            return (
              <button
                key={opt.id}
                type="button"
                onClick={() => {
                  setLayout(opt.id);
                  if (opt.id === "grid") setSpotlightId(null);
                  if (opt.id === "spotlight" && !spotlightId && tiles.length) {
                    setSpotlightId(tiles[0].id);
                  }
                }}
                className="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors"
                style={{
                  background: on ? "rgba(255,255,255,0.14)" : "transparent",
                  color: on ? "#fff" : "rgba(255,255,255,0.65)",
                }}
              >
                <span className="text-[11px]">{opt.icon}</span>
                {opt.label}
              </button>
            );
          })}
        </div>
      </header>

      {/* ── Content area ─────────────────────────────────────────────────── */}
      <div className="relative min-h-0 flex-1">
        {layout === "grid" ? (
          <VideoGrid tiles={pageTiles} onPinTile={onPinTile} />
        ) : (
          <VideoSpotlight
            tiles={tiles}
            spotlightId={spotlightId}
            onPinTile={onPinTile}
          />
        )}

        {/* Floating call controls */}
        <div className="pointer-events-none absolute inset-x-0 bottom-[22px] z-20 flex justify-center">
          <div className="pointer-events-auto">
            <VideoCallControls
              isMicEnabled={isMicEnabled}
              isCamEnabled={isCamEnabled}
              isScreenSharing={isScreenSharing}
              onToggleMic={() => void toggleMic()}
              onToggleCamera={() => void toggleCamera()}
              onStartShare={() => setShareDialogOpen(true)}
              onStopShare={() => void unpublishScreen()}
              onLeave={leaveVoice}
            />
          </div>
        </div>
      </div>

      <ScreenShareProfileDialog
        open={shareDialogOpen}
        onOpenChange={setShareDialogOpen}
        onConfirm={(profile) => void publishScreen(profile)}
      />

      {import.meta.env.DEV && <CallDebugPanel client={client} />}
    </div>
  );
}
