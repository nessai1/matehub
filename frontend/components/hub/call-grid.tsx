"use client";

import { useEffect, useRef } from "react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { MicOffIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import type { Participant } from "../../../packages/sdk-video/src";

export interface MemberInfo {
  displayName: string;
  avatarUrl?: string | null;
}

interface CallGridProps {
  participants: Participant[];
  localStream: MediaStream | null;
  currentUserId: string;
  isCamEnabled: boolean;
  isMicEnabled: boolean;
  memberInfo: Record<string, MemberInfo>;
}

export function CallGrid({
  participants,
  localStream,
  currentUserId,
  isCamEnabled,
  isMicEnabled,
  memberInfo,
}: CallGridProps) {
  const localInfo = memberInfo[currentUserId];
  const localVideoTrack = isCamEnabled
    ? localStream?.getVideoTracks()[0] ?? null
    : null;

  const tiles = [
    {
      id: "local",
      userId: currentUserId,
      displayName: localInfo?.displayName ?? currentUserId,
      avatarUrl: localInfo?.avatarUrl ?? null,
      audioTrack: null, // never play back our own mic — feedback loop
      videoTrack: localVideoTrack,
      isMicMuted: !isMicEnabled,
      isSpeaking: false,
      isLocal: true,
    },
    ...participants.map((p) => {
      const info = memberInfo[p.userId];
      return {
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
    }),
  ];

  const gridCols =
    tiles.length <= 1
      ? "grid-cols-1"
      : tiles.length <= 4
        ? "grid-cols-2"
        : "grid-cols-3";

  return (
    <div className={cn("grid gap-2 p-3", gridCols)}>
      {tiles.map((tile) => (
        <ParticipantTile key={tile.id} {...tile} />
      ))}
    </div>
  );
}

interface ParticipantTileProps {
  id: string;
  userId: string;
  displayName: string;
  avatarUrl: string | null;
  audioTrack: MediaStreamTrack | null;
  videoTrack: MediaStreamTrack | null;
  isMicMuted: boolean;
  isSpeaking: boolean;
  isLocal: boolean;
}

function ParticipantTile({
  displayName,
  avatarUrl,
  audioTrack,
  videoTrack,
  isMicMuted,
  isSpeaking,
  isLocal,
}: ParticipantTileProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const audioRef = useRef<HTMLAudioElement>(null);

  // Attach video track to the <video> element. Rebuilt per track so
  // React's deps fire on track replacement (replaceTrack keeps object id
  // but we treat it as a new source here for simplicity).
  useEffect(() => {
    const el = videoRef.current;
    if (!el) return;
    if (videoTrack) {
      el.srcObject = new MediaStream([videoTrack]);
      el.play().catch(() => {});
    } else {
      el.srcObject = null;
    }
  }, [videoTrack]);

  // Attach audio track to a dedicated hidden <audio> element. Without
  // a media sink Chrome never primes the audio decoder — packets arrive
  // but are discarded and jitterBufferEmittedCount stays at 0. Skipped
  // for the local tile (would feed our own mic back into the speakers).
  useEffect(() => {
    const el = audioRef.current;
    if (!el) return;
    if (audioTrack && !isLocal) {
      el.srcObject = new MediaStream([audioTrack]);
      el.play().catch(() => {});
    } else {
      el.srcObject = null;
    }
  }, [audioTrack, isLocal]);

  const initials = displayName
    .split(" ")
    .map((n) => n[0])
    .join("")
    .toUpperCase()
    .slice(0, 2);

  const hasVideo = !!videoTrack;

  return (
    <div
      className={cn(
        "relative flex aspect-video items-center justify-center overflow-hidden rounded-xl bg-muted/50 transition-all duration-200",
        isSpeaking &&
          "ring-2 ring-emerald-500 ring-offset-2 ring-offset-background",
      )}
    >
      {hasVideo ? (
        <video
          ref={videoRef}
          autoPlay
          playsInline
          muted={isLocal}
          className={cn(
            "h-full w-full object-cover will-change-transform",
            isLocal && "-scale-x-100",
          )}
        />
      ) : (
        <Avatar className="h-16 w-16">
          {avatarUrl && <AvatarImage src={avatarUrl} />}
          <AvatarFallback className="bg-primary/15 text-2xl font-semibold text-primary">
            {initials}
          </AvatarFallback>
        </Avatar>
      )}

      {/* Hidden audio sink. Always present for remote tiles so the decoder
          runs regardless of the video-element mount state. */}
      {!isLocal && <audio ref={audioRef} autoPlay playsInline hidden />}

      {/* Name badge + mic muted indicator */}
      <div className="absolute bottom-2 left-2 flex items-center gap-1.5 rounded-md bg-black/60 px-2 py-0.5">
        {isMicMuted && (
          <MicOffIcon className="h-3 w-3 text-red-400" />
        )}
        <span className="text-xs font-medium text-white">
          {isLocal ? `${displayName} (You)` : displayName}
        </span>
      </div>
    </div>
  );
}
