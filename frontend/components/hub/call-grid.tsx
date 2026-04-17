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

  const tiles = [
    {
      id: "local",
      userId: currentUserId,
      displayName: localInfo?.displayName ?? currentUserId,
      avatarUrl: localInfo?.avatarUrl ?? null,
      stream: localStream,
      hasVideo: isCamEnabled,
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
        stream: p.stream,
        hasVideo: p.videoTrack?.enabled ?? false,
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
  stream: MediaStream | null;
  hasVideo: boolean;
  isMicMuted: boolean;
  isSpeaking: boolean;
  isLocal: boolean;
}

function ParticipantTile({
  displayName,
  avatarUrl,
  stream,
  hasVideo,
  isMicMuted,
  isSpeaking,
  isLocal,
}: ParticipantTileProps) {
  const videoRef = useRef<HTMLVideoElement>(null);

  useEffect(() => {
    if (videoRef.current) {
      if (stream && hasVideo) {
        videoRef.current.srcObject = stream;
        videoRef.current.play().catch(() => {});
      } else {
        videoRef.current.srcObject = null;
      }
    }
  }, [stream, hasVideo]);

  const initials = displayName
    .split(" ")
    .map((n) => n[0])
    .join("")
    .toUpperCase()
    .slice(0, 2);

  return (
    <div
      className={cn(
        "relative flex aspect-video items-center justify-center overflow-hidden rounded-xl bg-muted/50 transition-all duration-200",
        isSpeaking &&
          "ring-2 ring-emerald-500 ring-offset-2 ring-offset-background",
      )}
    >
      {hasVideo && stream ? (
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
