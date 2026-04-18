"use client";

import { useState } from "react";
import { Gamepad2, Monitor, FileText } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { ScreenShareProfile } from "../../../packages/sdk-video/src";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (profile: ScreenShareProfile) => void;
}

interface ProfileOption {
  id: ScreenShareProfile;
  title: string;
  description: string;
  resolution: string;
  icon: typeof Monitor;
}

const PROFILES: ProfileOption[] = [
  {
    id: "gaming",
    title: "Gaming",
    description: "High motion, 60 fps. For gameplay demos.",
    resolution: "1080p · 6 Mbps",
    icon: Gamepad2,
  },
  {
    id: "standard",
    title: "Standard",
    description: "Balanced quality. For general screen sharing.",
    resolution: "720p · 2 Mbps",
    icon: Monitor,
  },
  {
    id: "detail",
    title: "Detail",
    description: "Sharp text. For documents and code.",
    resolution: "1440p · 5 fps",
    icon: FileText,
  },
];

export function ScreenShareProfileDialog({ open, onOpenChange, onConfirm }: Props) {
  const [selected, setSelected] = useState<ScreenShareProfile>("gaming");
  const isMac =
    typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Share your screen</DialogTitle>
          <DialogDescription>
            Pick a quality profile. Your browser will ask which window or
            screen to share next.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-2">
          {PROFILES.map((p) => {
            const active = selected === p.id;
            const Icon = p.icon;
            return (
              <button
                key={p.id}
                type="button"
                onClick={() => setSelected(p.id)}
                className={cn(
                  "flex items-start gap-3 rounded-lg border p-3 text-left transition",
                  active
                    ? "border-primary bg-primary/5"
                    : "border-border hover:bg-muted/50",
                )}
              >
                <Icon className="mt-0.5 h-5 w-5 shrink-0 text-primary" />
                <div className="flex-1">
                  <div className="flex items-center justify-between">
                    <span className="text-sm font-semibold">{p.title}</span>
                    <span className="font-mono text-[10px] text-muted-foreground">
                      {p.resolution}
                    </span>
                  </div>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    {p.description}
                  </p>
                </div>
              </button>
            );
          })}
        </div>

        {isMac && (
          <p className="rounded border border-amber-500/40 bg-amber-500/10 p-2 text-xs text-amber-600 dark:text-amber-400">
            On macOS, game/system audio can only be captured from a browser tab.
            Sharing a full screen will be video-only.
          </p>
        )}

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            onClick={() => {
              onConfirm(selected);
              onOpenChange(false);
            }}
          >
            Start sharing
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
