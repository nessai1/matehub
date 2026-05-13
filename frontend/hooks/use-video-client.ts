import { useCallback, useEffect, useRef, useState } from "react";
import {
  VideoClient,
  type Participant,
  type ScreenShareProfile,
  type VideoClientEvent,
} from "@matehub/sdk-video";
import { playCallSound } from "@/lib/call-sounds";
import { getTurnConfig } from "@/src/config";

// localStorage keys for cross-call device persistence (MAT-17). Browser-
// scoped, not user-scoped — running two users on one browser ties them to
// the same default, which is the right tradeoff: device choice belongs to
// the human at the keyboard, not the credentials they're logged in with.
const MIC_DEVICE_STORAGE_KEY = "matehub.videoCall.micDeviceId";
const CAMERA_DEVICE_STORAGE_KEY = "matehub.videoCall.cameraDeviceId";

function readPersistedDevice(key: string): string | null {
  // SSR-safe: Next.js may render this hook on the server during hydration
  // (the hook itself doesn't gate on `typeof window`, the call sites do).
  // localStorage access during SSR throws ReferenceError, hence the guard.
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writePersistedDevice(key: string, deviceId: string) {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(key, deviceId);
  } catch {
    // Quota / private-mode / cookies-disabled paths. Persistence is a nicety,
    // not a correctness requirement — swallow.
  }
}

interface UseVideoClientOptions {
  serverUrl: string;
  sessionId: string;
  userId: string;
  userUuid?: string;
  token: string;
  /** Fires when the SFU explicitly kicks us — e.g. the same user joined
   *  this call from another tab. Caller should leave the call UI
   *  cleanly (call leaveVoice) and surface a message to the user. */
  onForceDisconnected?: (reason: string) => void;
}

interface UseVideoClientReturn {
  participants: Participant[];
  localStream: MediaStream | null;
  /** Local screen-video track for self-preview (null when not sharing). */
  localScreenVideoTrack: MediaStreamTrack | null;
  isConnected: boolean;
  isMicEnabled: boolean;
  isCamEnabled: boolean;
  isScreenSharing: boolean;
  toggleMic: () => Promise<void>;
  toggleCamera: () => Promise<void>;
  publishScreen: (profile: ScreenShareProfile) => Promise<void>;
  unpublishScreen: () => Promise<void>;
  connect: () => Promise<void>;
  disconnect: () => void;
  error: string | null;
  /** Underlying SDK client — exposed for debug tooling. Null when not connected. */
  client: VideoClient | null;
  // ── Device selection ──
  audioInputs: MediaDeviceInfo[];
  videoInputs: MediaDeviceInfo[];
  currentMicDeviceId: string | null;
  currentCameraDeviceId: string | null;
  setMicDevice: (deviceId: string) => Promise<void>;
  setCameraDevice: (deviceId: string) => Promise<void>;
  refreshDevices: () => Promise<void>;
}

export function useVideoClient(
  opts: UseVideoClientOptions | null,
): UseVideoClientReturn {
  const clientRef = useRef<VideoClient | null>(null);
  // Mirror client ref in state so consumers (debug panel) rerender when it appears.
  const [client, setClient] = useState<VideoClient | null>(null);
  const [participants, setParticipants] = useState<Participant[]>([]);
  const [localStream, setLocalStream] = useState<MediaStream | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const [isMicEnabled, setIsMicEnabled] = useState(false);
  const [isCamEnabled, setIsCamEnabled] = useState(false);
  const [isScreenSharing, setIsScreenSharing] = useState(false);
  const [localScreenVideoTrack, setLocalScreenVideoTrack] =
    useState<MediaStreamTrack | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [audioInputs, setAudioInputs] = useState<MediaDeviceInfo[]>([]);
  const [videoInputs, setVideoInputs] = useState<MediaDeviceInfo[]>([]);
  const [currentMicDeviceId, setCurrentMicDeviceId] = useState<string | null>(
    null,
  );
  const [currentCameraDeviceId, setCurrentCameraDeviceId] = useState<
    string | null
  >(null);

  // Two refs that together solve the race between mute signaling (fast, WS)
  // and track establishment (slow, ICE+SDP). Either can arrive first.
  // Both are updated synchronously, independent of React render cycle.
  const cameraOnRef = useRef(new Set<string>());
  const videoTrackRef = useRef(new Map<string, MediaStreamTrack>());

  // Update participant in list (immutable)
  const updateParticipants = useCallback(
    (fn: (prev: Participant[]) => Participant[]) => {
      setParticipants(fn);
    },
    [],
  );

  const connect = useCallback(async () => {
    if (!opts || clientRef.current) return;

    const iceServers: RTCIceServer[] = [
      { urls: "stun:stun.l.google.com:19302" },
    ];
    const turn = getTurnConfig();
    if (turn) {
      iceServers.push(turn);
    }

    const client = new VideoClient({
      serverUrl: opts.serverUrl,
      sessionId: opts.sessionId,
      userId: opts.userId,
      userUuid: opts.userUuid,
      token: opts.token,
      iceServers,
    });

    client.on((event: VideoClientEvent) => {
      switch (event.type) {
        case "connected":
          setIsConnected(true);
          setError(null);
          // Sync media state -- SDK may have enabled mic/cam during connect
          setIsMicEnabled(client.isMicEnabled);
          setIsCamEnabled(client.isCamEnabled);
          setLocalStream(client.getLocalStream());
          setCurrentMicDeviceId(client.getCurrentMicDeviceId());
          setCurrentCameraDeviceId(client.getCurrentCameraDeviceId());
          break;
        case "participant_joined":
          // Audio cue (MAT-18): reuse the existing join chime so a
          // teammate slipping into the call mid-stream isn't invisible.
          // The 15% volume keeps it from clipping into ongoing speech.
          playCallSound("join_call");
          updateParticipants((prev) => [...prev, event.participant]);
          break;
        case "participant_left":
          playCallSound("leave_call");
          cameraOnRef.current.delete(event.participantId);
          videoTrackRef.current.delete(event.participantId);
          updateParticipants((prev) =>
            prev.filter((p) => p.participantId !== event.participantId),
          );
          break;
        case "track_added":
          // MAT-18: remote screen-share starting deserves the same cue
          // we play for the local side — different listener, same event
          // semantically. Camera tracks are quiet because they fire on
          // every renegotiation, not just join.
          if (event.source === "screen" && event.kind === "video") {
            playCallSound("show_desktop");
          }
          // Camera video ref — screen tracks don't need the mute-race workaround
          // since they're not gated on a mute signal, they exist or they don't.
          if (event.source === "camera" && event.kind === "video") {
            videoTrackRef.current.set(event.participantId, event.track);
          }
          updateParticipants((prev) =>
            prev.map((p) => {
              if (p.participantId !== event.participantId) return p;
              const next = { ...p, stream: event.stream };
              if (event.source === "camera" && event.kind === "audio") {
                next.audioTrack = event.track;
              } else if (event.source === "camera" && event.kind === "video") {
                // Respect mute-signal race (fixed in P1-P2 of Stage 2).
                next.videoTrack = cameraOnRef.current.has(event.participantId)
                  ? event.track
                  : null;
              } else if (event.source === "screen" && event.kind === "video") {
                next.screenVideoTrack = event.track;
              } else if (event.source === "screen" && event.kind === "audio") {
                next.screenAudioTrack = event.track;
              }
              return next;
            }),
          );
          break;
        case "track_removed":
          // MAT-18: complement to the start cue — remote stopped sharing
          // their screen, surface it with the same disable_desktop chime
          // we already use for the local side.
          if (event.source === "screen" && event.kind === "video") {
            playCallSound("disable_desktop");
          }
          updateParticipants((prev) =>
            prev.map((p) => {
              if (p.participantId !== event.participantId) return p;
              const next = { ...p };
              if (event.source === "camera" && event.kind === "audio") next.audioTrack = null;
              else if (event.source === "camera" && event.kind === "video") next.videoTrack = null;
              else if (event.source === "screen" && event.kind === "video") next.screenVideoTrack = null;
              else if (event.source === "screen" && event.kind === "audio") next.screenAudioTrack = null;
              return next;
            }),
          );
          break;
        case "screen_share_started":
        case "screen_share_stopped":
          // UI layout switch is driven off participant.screenVideoTrack presence;
          // events are logged for debug only.
          break;
        case "track_muted":
          // Record signaling state in ref (survives the race with track_added)
          if (event.trackKind === "video") {
            if (event.muted) {
              cameraOnRef.current.delete(event.participantId);
            } else {
              cameraOnRef.current.add(event.participantId);
            }
          }
          updateParticipants((prev) =>
            prev.map((p) => {
              if (p.participantId !== event.participantId) return p;
              if (event.trackKind === "video") {
                const track = event.muted
                  ? null
                  : videoTrackRef.current.get(event.participantId) ?? null;
                return { ...p, videoTrack: track };
              }
              if (event.trackKind === "audio") {
                return { ...p, isMicMuted: event.muted };
              }
              return p;
            }),
          );
          break;
        case "speaking_changed":
          updateParticipants((prev) =>
            prev.map((p) =>
              p.participantId === event.participantId
                ? { ...p, isSpeaking: event.speaking }
                : p,
            ),
          );
          break;
        case "disconnected":
          setIsConnected(false);
          break;
        case "force_disconnected":
          // Server replaced our session (multi-tab collision). Hop back
          // to "not in a call" state and let the caller show a toast +
          // leave the call view.
          setIsConnected(false);
          opts?.onForceDisconnected?.(event.reason);
          break;
        case "error":
          setError(event.message);
          break;
      }
    });

    clientRef.current = client;
    setClient(client);
    await client.connect();
  }, [opts, updateParticipants]);

  const disconnect = useCallback(() => {
    clientRef.current?.disconnect();
    clientRef.current = null;
    setClient(null);
    setIsConnected(false);
    setParticipants([]);
    setLocalStream(null);
    setIsMicEnabled(false);
    setIsCamEnabled(false);
    setIsScreenSharing(false);
    setLocalScreenVideoTrack(null);
    setError(null);
    cameraOnRef.current.clear();
    videoTrackRef.current.clear();
  }, []);

  // Apply the persisted device choice (MAT-17) when a freshly-enabled
  // track lands on the default device. The SDK's `enableMic` /
  // `enableCamera` always reach for the platform default — they have no
  // hook to honour a previous selection. Wrapping the toggle here is the
  // cheap place to splice that in: after the toggle returns enabled=true
  // we check whether the saved deviceId differs from the live one, and
  // if so swap via `setMicDevice` / `setCameraDevice` (which does
  // replaceTrack on the existing sender — no SDP churn).
  //
  // The saved id may be stale: a previously-used USB headset that's no
  // longer plugged in. `listDevices()` enumerates what's available right
  // now; we silently skip the switch if the saved id isn't in the list.
  const applyPersistedMic = useCallback(async (client: VideoClient) => {
    const saved = readPersistedDevice(MIC_DEVICE_STORAGE_KEY);
    if (!saved) return;
    const current = client.getCurrentMicDeviceId();
    if (current === saved) return;
    try {
      const devices = await client.listDevices();
      if (!devices.audioInputs.some((d) => d.deviceId === saved)) return;
      await client.setMicDevice(saved);
      setCurrentMicDeviceId(client.getCurrentMicDeviceId());
    } catch (e) {
      // Device may have been pulled between enumerate and switch.
      console.warn("applyPersistedMic failed", e);
    }
  }, []);

  const applyPersistedCamera = useCallback(async (client: VideoClient) => {
    const saved = readPersistedDevice(CAMERA_DEVICE_STORAGE_KEY);
    if (!saved) return;
    const current = client.getCurrentCameraDeviceId();
    if (current === saved) return;
    try {
      const devices = await client.listDevices();
      if (!devices.videoInputs.some((d) => d.deviceId === saved)) return;
      await client.setCameraDevice(saved);
      setCurrentCameraDeviceId(client.getCurrentCameraDeviceId());
    } catch (e) {
      console.warn("applyPersistedCamera failed", e);
    }
  }, []);

  const toggleMic = useCallback(
    async () => {
      const client = clientRef.current;
      if (!client) return;
      const enabled = await client.toggleMic();
      setIsMicEnabled(enabled);
      // MAT-18 second pass: audio confirmation on every mic/cam toggle.
      // Played AFTER the SDK call resolves so the cue confirms the
      // state actually changed (otherwise a fast-clicker hears the
      // "click" before getUserMedia finishes — confusing if permission
      // is still being prompted).
      playCallSound(enabled ? "enable" : "disable");
      // Only swap on enable: a mute toggle shouldn't churn devices.
      if (enabled) {
        await applyPersistedMic(client);
      }
    },
    [applyPersistedMic],
  );

  const toggleCamera = useCallback(
    async () => {
      const client = clientRef.current;
      if (!client) return;
      const enabled = await client.toggleCamera();
      setIsCamEnabled(enabled);
      setLocalStream(client.getLocalStream());
      playCallSound(enabled ? "enable" : "disable");
      if (enabled) {
        await applyPersistedCamera(client);
        setLocalStream(client.getLocalStream());
      }
    },
    [applyPersistedCamera],
  );

  const publishScreen = useCallback(async (profile: ScreenShareProfile) => {
    const client = clientRef.current;
    if (!client) return;
    try {
      await client.publishScreen(profile);
      setIsScreenSharing(client.isScreenSharing);
      const track = client.getScreenVideoTrack();
      setLocalScreenVideoTrack(track);
      // SFU doesn't loop the publisher's own stream back, so the self-view
      // below reads directly from the local track. `onended` fires when the
      // user hits "Stop sharing" in Chrome — keep the state in sync.
      if (track) {
        playCallSound("show_desktop");
        track.addEventListener(
          "ended",
          () => {
            setLocalScreenVideoTrack(null);
            playCallSound("disable_desktop");
          },
          { once: true },
        );
      }
    } catch (e) {
      // User cancelled picker, or permission denied — not a fatal error,
      // just don't flip the sharing flag.
      console.warn("publishScreen failed", e);
    }
  }, []);

  const unpublishScreen = useCallback(async () => {
    const client = clientRef.current;
    if (!client) return;
    // The cue is played from the track's `onended` handler — that fires both
    // when we explicitly stop the track here AND when the user hits Chrome's
    // own "Stop sharing" bar, so attaching the sound there avoids dedup.
    await client.unpublishScreen();
    setIsScreenSharing(client.isScreenSharing);
    setLocalScreenVideoTrack(null);
  }, []);

  // ── Device enumeration / switching ────────────────────────────────────
  // Labels are empty until permission is granted. We populate the lists
  // both right after connect (when getUserMedia has run) and on every
  // `devicechange` event the browser fires (USB plug, BT pairing, etc).

  const refreshDevices = useCallback(async () => {
    const client = clientRef.current;
    if (!client) return;
    try {
      const { audioInputs, videoInputs } = await client.listDevices();
      setAudioInputs(audioInputs);
      setVideoInputs(videoInputs);
    } catch (e) {
      console.warn("listDevices failed", e);
    }
  }, []);

  useEffect(() => {
    if (!isConnected) return;
    void refreshDevices();
    const onChange = () => void refreshDevices();
    navigator.mediaDevices.addEventListener("devicechange", onChange);
    return () => {
      navigator.mediaDevices.removeEventListener("devicechange", onChange);
    };
  }, [isConnected, refreshDevices]);

  const setMicDevice = useCallback(async (deviceId: string) => {
    const client = clientRef.current;
    if (!client) return;
    await client.setMicDevice(deviceId);
    const settled = client.getCurrentMicDeviceId();
    setCurrentMicDeviceId(settled);
    // Persist the *settled* id, not the requested one. They line up in the
    // happy path, but `setMicDevice` may reach for a fallback if the
    // requested device disappears mid-call (USB unplug during the
    // switch). Saving what the SDK actually picked keeps the next call's
    // restore consistent with reality.
    if (settled) {
      writePersistedDevice(MIC_DEVICE_STORAGE_KEY, settled);
    }
    // Force a new MediaStream reference so consumers' useMemo rebuilds —
    // mutating the existing one in-place doesn't trip referential equality.
    const s = client.getLocalStream();
    setLocalStream(s ? new MediaStream(s.getTracks()) : null);
  }, []);

  const setCameraDevice = useCallback(async (deviceId: string) => {
    const client = clientRef.current;
    if (!client) return;
    await client.setCameraDevice(deviceId);
    const settled = client.getCurrentCameraDeviceId();
    setCurrentCameraDeviceId(settled);
    if (settled) {
      writePersistedDevice(CAMERA_DEVICE_STORAGE_KEY, settled);
    }
    const s = client.getLocalStream();
    setLocalStream(s ? new MediaStream(s.getTracks()) : null);
  }, []);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      clientRef.current?.disconnect();
      clientRef.current = null;
    };
  }, []);

  return {
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
    audioInputs,
    videoInputs,
    currentMicDeviceId,
    currentCameraDeviceId,
    setMicDevice,
    setCameraDevice,
    refreshDevices,
  };
}
