"use client";

import {
  Mic,
  MicOff,
  Monitor,
  MonitorOff,
  PhoneOff,
  Video,
  VideoOff,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

interface CallControlsProps {
  isMicEnabled: boolean;
  isCamEnabled: boolean;
  isScreenSharing: boolean;
  onToggleMic: () => void;
  onToggleCamera: () => void;
  onStartShare: () => void;
  onStopShare: () => void;
  onLeave: () => void;
}

export function CallControls({
  isMicEnabled,
  isCamEnabled,
  isScreenSharing,
  onToggleMic,
  onToggleCamera,
  onStartShare,
  onStopShare,
  onLeave,
}: CallControlsProps) {
  return (
    <TooltipProvider delayDuration={200}>
      <div className="flex items-center justify-center gap-2 border-t bg-card/80 px-4 py-2">
        <ControlButton
          icon={isMicEnabled ? Mic : MicOff}
          label={isMicEnabled ? "Mute" : "Unmute"}
          active={isMicEnabled}
          onClick={onToggleMic}
        />
        <ControlButton
          icon={isCamEnabled ? Video : VideoOff}
          label={isCamEnabled ? "Turn off camera" : "Turn on camera"}
          active={isCamEnabled}
          onClick={onToggleCamera}
        />
        <ControlButton
          icon={isScreenSharing ? MonitorOff : Monitor}
          label={isScreenSharing ? "Stop sharing" : "Share screen"}
          active={isScreenSharing}
          destructive={isScreenSharing}
          onClick={isScreenSharing ? onStopShare : onStartShare}
        />
        <div className="mx-2 h-6 w-px bg-border" />
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="destructive"
              size="sm"
              className="h-9 w-9 rounded-full p-0"
              onClick={onLeave}
            >
              <PhoneOff className="h-4 w-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>Leave call</TooltipContent>
        </Tooltip>
      </div>
    </TooltipProvider>
  );
}

function ControlButton({
  icon: Icon,
  label,
  active,
  destructive,
  onClick,
}: {
  icon: typeof Mic;
  label: string;
  active?: boolean;
  destructive?: boolean;
  onClick: () => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className={cn(
            "h-9 w-9 rounded-full p-0",
            active === false && "bg-muted text-muted-foreground",
            destructive && "bg-red-500/15 text-red-500 hover:bg-red-500/20 hover:text-red-500",
          )}
          onClick={onClick}
        >
          <Icon className="h-4 w-4" />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}
