"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { Mic, PhoneIncoming } from "lucide-react";
import { Button } from "@/components/ui/button";
import { CallGrid, type MemberInfo } from "@/components/hub/call-grid";
import { CallControls } from "@/components/hub/call-controls";
import { CallDebugPanel } from "@/components/hub/call-debug-panel";
import { ScreenShareProfileDialog } from "@/components/hub/screen-share-profile-dialog";
import { TextChannelView } from "@/components/hub/text-channel-view";
import { useVideoClient } from "@/hooks/use-video-client";
import { useMembers } from "@/hooks/use-members";
import { useAuth } from "@/lib/auth";

interface VoiceChannelViewProps {
  channelId: string;
  channelName: string;
}

const VIDEO_SERVER_URL =
  process.env.NEXT_PUBLIC_VIDEO_SERVER_URL ?? "http://localhost:4000";

const DEV_CHANNEL_IDS: Record<string, string> = {
  "voice-test": "00000000-0000-0000-0000-000000000101",
  "stage-test": "00000000-0000-0000-0000-000000000102",
};

export function VoiceChannelView({
  channelId,
  channelName,
}: VoiceChannelViewProps) {
  const { session } = useAuth();
  const { members } = useMembers();
  const username = session?.username ?? "anonymous";
  const token = session?.token ?? "";

  const [sessionId, setSessionId] = useState<string | null>(null);
  const [inCall, setInCall] = useState(false);
  const [shareDialogOpen, setShareDialogOpen] = useState(false);

  // Build memberInfo map for call grid (userId -> display name + avatar)
  const memberInfo = useMemo<Record<string, MemberInfo>>(() => {
    const map: Record<string, MemberInfo> = {};
    // Current user from session
    if (session) {
      map[session.username] = {
        displayName: session.displayName,
        avatarUrl: session.avatarUrl,
      };
    }
    // Remote users from members API
    for (const m of members) {
      map[m.username] = {
        displayName: m.display_name,
        avatarUrl: m.avatar_url,
      };
    }
    return map;
  }, [session, members]);

  const {
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
    connect,
    disconnect,
    error,
    client,
  } = useVideoClient(
    sessionId
      ? {
          serverUrl: VIDEO_SERVER_URL,
          sessionId,
          userId: username,
          token,
        }
      : null,
  );

  const joinCall = useCallback(async () => {
    try {
      const resp = await fetch(`${VIDEO_SERVER_URL}/v1/sessions`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          channel_id:
            DEV_CHANNEL_IDS[channelId] ??
            "00000000-0000-0000-0000-000000000100",
        }),
      });

      if (!resp.ok) {
        console.error("Failed to create session:", resp.status);
        return;
      }

      const data = await resp.json();
      setSessionId(data.session_id);
      setInCall(true);
    } catch (e) {
      console.error("Failed to join call:", e);
    }
  }, [channelId]);

  useEffect(() => {
    if (sessionId && inCall) {
      connect();
    }
  }, [sessionId, inCall, connect]);

  const leaveCall = useCallback(() => {
    disconnect();
    setInCall(false);
    setSessionId(null);
  }, [disconnect]);

  if (!inCall) {
    return (
      <div className="flex flex-1 flex-col">
        <div className="flex flex-1 flex-col items-center justify-center gap-4">
          <div className="flex h-16 w-16 items-center justify-center rounded-2xl bg-primary/10">
            <Mic className="h-8 w-8 text-primary" />
          </div>
          <div className="text-center">
            <h2 className="text-lg font-semibold">{channelName}</h2>
            <p className="mt-1 text-sm text-muted-foreground">
              Voice channel -- click to join
            </p>
          </div>
          <Button onClick={joinCall} className="gap-2">
            <PhoneIncoming className="h-4 w-4" />
            Join Voice
          </Button>
          {error && <p className="text-sm text-destructive">{error}</p>}
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-1 flex-col overflow-hidden">
      <div className="flex flex-1 flex-col border-b">
        <div className="flex h-10 shrink-0 items-center border-b px-4">
          <Mic className="mr-1.5 h-4 w-4 text-emerald-500" />
          <span className="text-sm font-medium">{channelName}</span>
          <span className="ml-2 text-xs text-muted-foreground">
            {isConnected ? "Connected" : "Connecting..."}
          </span>
          <span className="ml-auto text-xs text-muted-foreground">
            {participants.length + 1} in call
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
          onLeave={leaveCall}
        />
      </div>

      <div className="flex flex-1 flex-col overflow-hidden">
        <TextChannelView channelId={channelId} channelName={channelName} />
      </div>

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
