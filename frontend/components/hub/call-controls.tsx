"use client";

import { Mic, MicOff, Video, VideoOff, Monitor, PhoneOff } from "lucide-react";
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
  onToggleMic: () => void;
  onToggleCamera: () => void;
  onScreenShare: () => void;
  onLeave: () => void;
}

export function CallControls({
  isMicEnabled,
  isCamEnabled,
  onToggleMic,
  onToggleCamera,
  onScreenShare,
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
          icon={Monitor}
          label="Share screen"
          onClick={onScreenShare}
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
  onClick,
}: {
  icon: typeof Mic;
  label: string;
  active?: boolean;
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
