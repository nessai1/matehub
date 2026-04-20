"use client";

import { useState } from "react";
import {
  Mic,
  MicOff,
  Monitor,
  MonitorOff,
  PhoneOff,
  Video,
  VideoOff,
} from "lucide-react";

interface VideoCallControlsProps {
  isMicEnabled: boolean;
  isCamEnabled: boolean;
  isScreenSharing: boolean;
  onToggleMic: () => void;
  onToggleCamera: () => void;
  onStartShare: () => void;
  onStopShare: () => void;
  onLeave: () => void;
}

/**
 * Floating pill of call controls. Visual styling is pixel-matched to the
 * handoff (dark glass, 42px buttons, solid-red leave). Icons are project
 * lucide.
 */
export function VideoCallControls({
  isMicEnabled,
  isCamEnabled,
  isScreenSharing,
  onToggleMic,
  onToggleCamera,
  onStartShare,
  onStopShare,
  onLeave,
}: VideoCallControlsProps) {
  return (
    <div
      className="inline-flex items-center gap-2 rounded-full p-[7px]"
      style={{
        background: "rgba(10,12,20,0.6)",
        backdropFilter: "blur(16px)",
        border: "1px solid rgba(255,255,255,0.08)",
        boxShadow: "0 12px 40px rgba(0,0,0,0.45)",
      }}
    >
      <ControlBtn
        active={isMicEnabled}
        onClick={onToggleMic}
        title={isMicEnabled ? "Mute" : "Unmute"}
      >
        {isMicEnabled ? <Mic className="h-4 w-4" /> : <MicOff className="h-4 w-4" />}
      </ControlBtn>

      <ControlBtn
        active={isCamEnabled}
        onClick={onToggleCamera}
        title={isCamEnabled ? "Turn off camera" : "Turn on camera"}
      >
        {isCamEnabled ? <Video className="h-4 w-4" /> : <VideoOff className="h-4 w-4" />}
      </ControlBtn>

      <ControlBtn
        active={isScreenSharing}
        onClick={isScreenSharing ? onStopShare : onStartShare}
        title={isScreenSharing ? "Stop sharing" : "Share screen"}
      >
        {isScreenSharing ? (
          <MonitorOff className="h-4 w-4" />
        ) : (
          <Monitor className="h-4 w-4" />
        )}
      </ControlBtn>

      <ControlBtn danger onClick={onLeave} title="Leave call">
        <PhoneOff className="h-4 w-4" />
      </ControlBtn>
    </div>
  );
}

function ControlBtn({
  children,
  active,
  danger,
  onClick,
  title,
}: {
  children: React.ReactNode;
  active?: boolean;
  danger?: boolean;
  onClick: () => void;
  title: string;
}) {
  const [hover, setHover] = useState(false);

  const style: React.CSSProperties = danger
    ? {
        background: hover ? "oklch(0.70 0.24 25)" : "oklch(0.62 0.22 25)",
        color: "#fff",
        border: "none",
      }
    : {
        background: hover
          ? "rgba(255,255,255,0.22)"
          : active
            ? "rgba(255,255,255,0.14)"
            : "rgba(255,255,255,0.06)",
        color: active || hover ? "#fff" : "rgba(255,255,255,0.85)",
        border: "1px solid rgba(255,255,255,0.08)",
      };

  return (
    <button
      type="button"
      onClick={onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onFocus={() => setHover(true)}
      onBlur={() => setHover(false)}
      title={title}
      aria-label={title}
      className="grid h-[42px] w-[42px] place-items-center rounded-full transition-colors"
      style={style}
    >
      {children}
    </button>
  );
}
