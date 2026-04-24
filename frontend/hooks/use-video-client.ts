import { useCallback, useEffect, useRef, useState } from "react";
import {
  VideoClient,
  type Participant,
  type ScreenShareProfile,
  type VideoClientEvent,
} from "../../packages/sdk-video/src";
import { playCallSound } from "@/lib/call-sounds";

interface UseVideoClientOptions {
  serverUrl: string;
  sessionId: string;
  userId: string;
  userUuid?: string;
  token: string;
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

    const client = new VideoClient({
      serverUrl: opts.serverUrl,
      sessionId: opts.sessionId,
      userId: opts.userId,
      userUuid: opts.userUuid,
      token: opts.token,
      iceServers: [{ urls: "stun:stun.l.google.com:19302" }],
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
          break;
        case "participant_joined":
          updateParticipants((prev) => [...prev, event.participant]);
          break;
        case "participant_left":
          cameraOnRef.current.delete(event.participantId);
          videoTrackRef.current.delete(event.participantId);
          updateParticipants((prev) =>
            prev.filter((p) => p.participantId !== event.participantId),
          );
          break;
        case "track_added":
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

  const toggleMic = useCallback(async () => {
    const client = clientRef.current;
    if (!client) return;
    const enabled = await client.toggleMic();
    setIsMicEnabled(enabled);
  }, []);

  const toggleCamera = useCallback(async () => {
    const client = clientRef.current;
    if (!client) return;
    const enabled = await client.toggleCamera();
    setIsCamEnabled(enabled);
    setLocalStream(client.getLocalStream());
  }, []);

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
  };
}
