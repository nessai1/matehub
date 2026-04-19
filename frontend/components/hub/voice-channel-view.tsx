"use client";

import { useMemo, useState } from "react";
import { Mic } from "lucide-react";
import { CallGrid, type MemberInfo } from "@/components/hub/call-grid";
import { CallControls } from "@/components/hub/call-controls";
import { CallDebugPanel } from "@/components/hub/call-debug-panel";
import { ScreenShareProfileDialog } from "@/components/hub/screen-share-profile-dialog";
import { useVideoCall } from "@/contexts/video-call-context";
import { useMembers } from "@/hooks/use-members";
import { useAuth } from "@/lib/auth";

interface VoiceChannelViewProps {
  channelId: string;
  channelName: string;
}

/**
 * Presentational shell around the active voice call. All state lives in
 * <VideoCallProvider>; this component just reads it. Joining/leaving is
 * driven by the page-level router (the act of opening a voice URL auto-joins).
 */
export function VoiceChannelView({
  channelId,
  channelName,
}: VoiceChannelViewProps) {
  const { session } = useAuth();
  const { members } = useMembers();
  const username = session?.username ?? "anonymous";

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

  const [shareDialogOpen, setShareDialogOpen] = useState(false);

  const memberInfo = useMemo<Record<string, MemberInfo>>(() => {
    const map: Record<string, MemberInfo> = {};
    if (session) {
      map[session.username] = {
        displayName: session.displayName,
        avatarUrl: session.avatarUrl,
      };
    }
    for (const m of members) {
      map[m.username] = {
        displayName: m.display_name,
        avatarUrl: m.avatar_url,
      };
    }
    return map;
  }, [session, members]);

  // This shouldn't render for a channel we're not in — page.tsx guards that.
  // But if it does, keep the header lit and say "connecting".
  const showingActive = activeVoiceChannelId === channelId;

  return (
    <div className="flex flex-1 flex-col overflow-hidden">
      <div className="flex h-10 shrink-0 items-center border-b px-4">
        <Mic className="mr-1.5 h-4 w-4 text-emerald-500" />
        <span className="text-sm font-medium">{channelName}</span>
        <span className="ml-2 text-xs text-muted-foreground">
          {showingActive && isConnected
            ? "Connected"
            : showingActive
              ? "Connecting…"
              : "Not in call"}
        </span>
        <span className="ml-auto text-xs text-muted-foreground">
          {participants.length + (showingActive ? 1 : 0)} in call
        </span>
      </div>

      <div className="flex-1 overflow-auto">
        <CallGrid
          participants={participants}
          localStream={localStream}
          localScreenVideoTrack={localScreenVideoTrack}
          currentUserId={username}
          isCamEnabled={isCamEnabled}
          isMicEnabled={isMicEnabled}
          memberInfo={memberInfo}
        />
      </div>

      <CallControls
        isMicEnabled={isMicEnabled}
        isCamEnabled={isCamEnabled}
        isScreenSharing={isScreenSharing}
        onToggleMic={toggleMic}
        onToggleCamera={toggleCamera}
        onStartShare={() => setShareDialogOpen(true)}
        onStopShare={() => void unpublishScreen()}
        onLeave={leaveVoice}
      />

      <ScreenShareProfileDialog
        open={shareDialogOpen}
        onOpenChange={setShareDialogOpen}
        onConfirm={(profile) => void publishScreen(profile)}
      />

      {process.env.NODE_ENV === "development" && (
        <CallDebugPanel client={client} />
      )}
    </div>
  );
}
