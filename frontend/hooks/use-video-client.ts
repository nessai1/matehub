"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  VideoClient,
  type Participant,
  type VideoClientEvent,
} from "../../packages/sdk-video/src";

interface UseVideoClientOptions {
  serverUrl: string;
  sessionId: string;
  userId: string;
  token: string;
}

interface UseVideoClientReturn {
  participants: Participant[];
  localStream: MediaStream | null;
  isConnected: boolean;
  isMicEnabled: boolean;
  isCamEnabled: boolean;
  toggleMic: () => Promise<void>;
  toggleCamera: () => Promise<void>;
  startScreenShare: () => Promise<void>;
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
          // Save track in ref (independent of React state)
          if (event.track.kind === "video") {
            videoTrackRef.current.set(event.participantId, event.track);
          }
          updateParticipants((prev) =>
            prev.map((p) =>
              p.participantId === event.participantId
                ? {
                    ...p,
                    stream: event.stream,
                    audioTrack:
                      event.track.kind === "audio" ? event.track : p.audioTrack,
                    // Show video only if signaling says camera is on
                    videoTrack:
                      event.track.kind === "video"
                        ? cameraOnRef.current.has(event.participantId)
                          ? event.track
                          : null
                        : p.videoTrack,
                  }
                : p,
            ),
          );
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

  const startScreenShare = useCallback(async () => {
    const client = clientRef.current;
    if (!client) return;
    await client.startScreenShare();
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
    isConnected,
    isMicEnabled,
    isCamEnabled,
    toggleMic,
    toggleCamera,
    startScreenShare,
    connect,
    disconnect,
    error,
    client,
  };
}
