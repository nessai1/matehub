"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useVideoClient } from "@/hooks/use-video-client";
import { useAuth } from "@/lib/auth";
import { playCallSound } from "@/lib/call-sounds";
import type {
  Participant,
  ScreenShareProfile,
  VideoClient,
} from "../../packages/sdk-video/src";

const VIDEO_SERVER_URL =
  process.env.NEXT_PUBLIC_VIDEO_SERVER_URL ?? "http://localhost:4000";

// Dev-only channel → session mapping. In production session_id would come from
// a proper backend mapping service; the SFU is happy to create/return sessions
// by channel_id, so we just feed it a stable UUID-shaped string.
const DEV_CHANNEL_IDS: Record<string, string> = {
  "voice-test": "00000000-0000-0000-0000-000000000101",
  "stage-test": "00000000-0000-0000-0000-000000000102",
};

function resolveChannelUuid(channelId: string): string {
  return (
    DEV_CHANNEL_IDS[channelId] ?? "00000000-0000-0000-0000-000000000100"
  );
}

interface VideoCallContextValue {
  /** The voice channel the user is currently CONNECTED to (not just viewing). */
  activeVoiceChannelId: string | null;

  // — Re-exported useVideoClient fields —
  client: VideoClient | null;
  participants: Participant[];
  localStream: MediaStream | null;
  localScreenVideoTrack: MediaStreamTrack | null;
  isConnected: boolean;
  isMicEnabled: boolean;
  isCamEnabled: boolean;
  isScreenSharing: boolean;
  toggleMic: () => Promise<void>;
  toggleCamera: () => Promise<void>;
  publishScreen: (profile: ScreenShareProfile) => Promise<void>;
  unpublishScreen: () => Promise<void>;
  error: string | null;

  /** Join a voice channel by its ID. Leaves the current call first if any. */
  joinVoice: (channelId: string) => Promise<void>;
  /** Leave the current call (noop if not in one). */
  leaveVoice: () => void;
}

const VideoCallContext = createContext<VideoCallContextValue | null>(null);

export function useVideoCall(): VideoCallContextValue {
  const ctx = useContext(VideoCallContext);
  if (!ctx) {
    throw new Error("useVideoCall must be used within <VideoCallProvider>");
  }
  return ctx;
}

export function VideoCallProvider({ children }: { children: ReactNode }) {
  const { session } = useAuth();
  // Channel the user is connected to. When null → not in a call.
  const [activeVoiceChannelId, setActiveVoiceChannelId] = useState<string | null>(
    null,
  );
  // Session id allocated by the SFU for that channel.
  const [sessionId, setSessionId] = useState<string | null>(null);

  const videoOpts = useMemo(() => {
    if (!sessionId || !session) return null;
    return {
      serverUrl: VIDEO_SERVER_URL,
      sessionId,
      userId: session.username,
      token: session.token,
    };
  }, [sessionId, session]);

  const vc = useVideoClient(videoOpts);

  // Auto-connect once the session id lands.
  useEffect(() => {
    if (sessionId && activeVoiceChannelId) {
      void vc.connect();
    }
    // vc.connect is stable by useCallback inside the hook
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, activeVoiceChannelId]);

  const joinVoice = useCallback(
    async (channelId: string) => {
      // Already in this call? Noop.
      if (activeVoiceChannelId === channelId) return;

      // Leaving a previous call first (keeps media state clean and avoids
      // two simultaneous PCs sharing the session hook).
      if (activeVoiceChannelId) {
        vc.disconnect();
        setSessionId(null);
      }

      try {
        const resp = await fetch(`${VIDEO_SERVER_URL}/v1/sessions`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ channel_id: resolveChannelUuid(channelId) }),
        });
        if (!resp.ok) {
          console.error("joinVoice: create session failed", resp.status);
          return;
        }
        const data = await resp.json();
        setSessionId(data.session_id);
        setActiveVoiceChannelId(channelId);
        playCallSound("join_call");
      } catch (e) {
        console.error("joinVoice: network error", e);
      }
    },
    [activeVoiceChannelId, vc],
  );

  const leaveVoice = useCallback(() => {
    if (!activeVoiceChannelId) return;
    playCallSound("leave_call");
    vc.disconnect();
    setSessionId(null);
    setActiveVoiceChannelId(null);
  }, [activeVoiceChannelId, vc]);

  const value = useMemo<VideoCallContextValue>(
    () => ({
      activeVoiceChannelId,
      client: vc.client,
      participants: vc.participants,
      localStream: vc.localStream,
      localScreenVideoTrack: vc.localScreenVideoTrack,
      isConnected: vc.isConnected,
      isMicEnabled: vc.isMicEnabled,
      isCamEnabled: vc.isCamEnabled,
      isScreenSharing: vc.isScreenSharing,
      toggleMic: vc.toggleMic,
      toggleCamera: vc.toggleCamera,
      publishScreen: vc.publishScreen,
      unpublishScreen: vc.unpublishScreen,
      error: vc.error,
      joinVoice,
      leaveVoice,
    }),
    [activeVoiceChannelId, vc, joinVoice, leaveVoice],
  );

  return (
    <VideoCallContext.Provider value={value}>
      {children}
    </VideoCallContext.Provider>
  );
}
