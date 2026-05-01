import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { toast } from "sonner";
import { useVideoClient } from "@/hooks/use-video-client";
import { useAuth } from "@/lib/auth";
import { playCallSound } from "@/lib/call-sounds";
import { t } from "@/i18n";
import type {
  Participant,
  ScreenShareProfile,
  VideoClient,
} from "../../packages/sdk-video/src";

const VIDEO_SERVER_URL = "/api/video";

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

  // ── Device picker ──
  audioInputs: MediaDeviceInfo[];
  videoInputs: MediaDeviceInfo[];
  currentMicDeviceId: string | null;
  currentCameraDeviceId: string | null;
  setMicDevice: (deviceId: string) => Promise<void>;
  setCameraDevice: (deviceId: string) => Promise<void>;
  refreshDevices: () => Promise<void>;

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
  // Set when SFU kicks us via force_disconnected; useEffect below reacts
  // to it (call leaveVoice + toast). Going through state — instead of
  // calling leaveVoice directly from the SDK callback — keeps the closure
  // dependencies sane (videoOpts otherwise would have to depend on the
  // current leaveVoice, recreating on every state change).
  const [forceKickReason, setForceKickReason] = useState<string | null>(null);

  const videoOpts = useMemo(() => {
    if (!sessionId || !session) return null;
    return {
      serverUrl: VIDEO_SERVER_URL,
      sessionId,
      // Canonical Snowflake user id — flows into SFU participant messages and
      // into voice-occupancy NATS events that the hub service matches against
      // `users.id`. No more display-name/uuid split.
      userId: session.userId,
      token: session.token,
      // Stable React setter — no closure freshness concerns.
      onForceDisconnected: setForceKickReason,
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

      if (!session) return;

      try {
        const resp = await fetch(`${VIDEO_SERVER_URL}/v1/sessions`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            channel_id: channelId,
            hub_id: session.hubId,
          }),
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
    [activeVoiceChannelId, vc, session],
  );

  const leaveVoice = useCallback(() => {
    if (!activeVoiceChannelId) return;
    playCallSound("leave_call");
    vc.disconnect();
    setSessionId(null);
    setActiveVoiceChannelId(null);
  }, [activeVoiceChannelId, vc]);

  // React to force-disconnect (multi-tab collision): leave the call view
  // cleanly and tell the user why. The reason currently has one value
  // ("joined_elsewhere") but the indirection costs nothing — server can
  // ship more reasons (admin kick, etc.) without FE changes here.
  useEffect(() => {
    if (forceKickReason === null) return;
    leaveVoice();
    if (forceKickReason === "joined_elsewhere") {
      toast.warning(t("Disconnected from call"), {
        description: t(
          "You joined this call from another window. Only one tab can be active at a time.",
        ),
      });
    } else {
      toast.warning(t("Disconnected from call"), {
        description: forceKickReason,
      });
    }
    setForceKickReason(null);
  }, [forceKickReason, leaveVoice]);

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
      audioInputs: vc.audioInputs,
      videoInputs: vc.videoInputs,
      currentMicDeviceId: vc.currentMicDeviceId,
      currentCameraDeviceId: vc.currentCameraDeviceId,
      setMicDevice: vc.setMicDevice,
      setCameraDevice: vc.setCameraDevice,
      refreshDevices: vc.refreshDevices,
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
