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
import { ParticipantVolumeStore } from "@/hooks/use-participant-volume";
import { useAuth } from "@/lib/auth";
import { playCallSound } from "@/lib/call-sounds";
import { t } from "@/i18n";
import type {
  Participant,
  ScreenShareProfile,
  VideoClient,
} from "@matehub/sdk-video";

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
  toggleMic: (opts?: { silent?: boolean }) => Promise<void>;
  toggleCamera: (opts?: { silent?: boolean }) => Promise<void>;
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

  /** Discord-style "deafen": mute every incoming audio AND mute own mic.
   *  Re-undeafening doesn't auto-unmute the mic — the user can stay
   *  muted independently. State is local to this tab. */
  isDeafened: boolean;
  toggleDeafen: () => void;

  /** Per-participant audio volume control (MAT-14). Range 0.0–1.0,
   *  applied locally to the receiver's HTMLAudioElement.volume. Lookup
   *  by user_id. Backed by an external store (see
   *  `useParticipantVolume`) so a slider drag re-renders only the
   *  affected tile, not every consumer of useVideoCall. */
  participantVolumeStore: ParticipantVolumeStore;
  setParticipantVolume: (userId: string, volume: number) => void;

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
  // Deafen (MAT-15) — local to this tab. Not persisted across reloads
  // intentionally: a user who reloads expects to *hear* the call. If we
  // want to honour "stay deafened across reloads" later, plumb through
  // localStorage same way device persistence does.
  const [isDeafened, setIsDeafened] = useState(false);
  // Per-participant volume overrides (MAT-14). External store with
  // keyed subscriptions — see hooks/use-participant-volume.ts for the
  // rationale (split-context perf rework, review #4).
  //
  // `useState(() => new Store())` is the React-idiomatic way to get a
  // single instance for the provider's lifetime without tripping the
  // "no ref-read during render" lint. We never call the setter — the
  // store is the source of truth, React state just owns its
  // construction.
  const [participantVolumeStore] = useState(
    () => new ParticipantVolumeStore(),
  );

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
    // Reset per-call deafen + volume state. Carrying them between calls
    // would surprise the user — "why is X quiet in this completely
    // different channel where I never adjusted them?".
    setIsDeafened(false);
    participantVolumeStore.reset();
  }, [activeVoiceChannelId, vc, participantVolumeStore]);

  const toggleDeafen = useCallback(() => {
    setIsDeafened((prev) => {
      const next = !prev;
      // Cue right at the click for snappy feedback (toggleMic below
      // resolves through a renegotiation that can take 50-250ms; the
      // user shouldn't have to wait that long to hear the click).
      playCallSound(next ? "disable" : "enable");
      // Discord-style coupling: deafening also mutes your own mic. The
      // intuition is "stop participating in the call" — leaking your
      // side comments while you can't hear anyone is the failure mode
      // we're closing. Undeafening leaves the mic where the user last
      // set it (likely still muted, which is correct).
      //
      // `silent: true` suppresses toggleMic's own cue — without it, a
      // second playCallSound("disable") would rewind the same cached
      // <audio> element via currentTime=0 and produce a brief stutter.
      // See toggleMic in use-video-client.ts for the silent-flag
      // rationale.
      if (next && vc.isMicEnabled) {
        void vc.toggleMic({ silent: true });
      }
      return next;
    });
  }, [vc]);

  const setParticipantVolume = useCallback(
    (userId: string, volume: number) => {
      // Clamping + 1.0-as-delete logic lives in the store itself so
      // any future caller (settings page, admin override, etc.) gets
      // the same semantics without re-implementing them here.
      participantVolumeStore.set(userId, volume);
    },
    [participantVolumeStore],
  );

  // React to force-disconnect (multi-tab collision): leave the call view
  // cleanly and tell the user why. The reason currently has one value
  // ("joined_elsewhere") but the indirection costs nothing — server can
  // ship more reasons (admin kick, etc.) without FE changes here.
  useEffect(() => {
    if (forceKickReason === null) return;
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
    // setState calls are dispatched via a microtask so the effect body
    // stays side-effect-only (toast above is a side effect, not setState).
    queueMicrotask(() => {
      leaveVoice();
      setForceKickReason(null);
    });
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
      isDeafened,
      toggleDeafen,
      participantVolumeStore,
      setParticipantVolume,
      joinVoice,
      leaveVoice,
    }),
    [
      activeVoiceChannelId,
      vc,
      isDeafened,
      toggleDeafen,
      participantVolumeStore,
      setParticipantVolume,
      joinVoice,
      leaveVoice,
    ],
  );

  return (
    <VideoCallContext.Provider value={value}>
      {children}
    </VideoCallContext.Provider>
  );
}
