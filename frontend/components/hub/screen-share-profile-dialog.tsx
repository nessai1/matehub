import { useEffect, useState } from "react";
import { Gamepad2, Monitor, FileText, AppWindow } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";
import { t } from "@/i18n";
import {
  getNativeBridge,
  type NativeShareTarget,
} from "@/lib/native-bridge";
import type { ScreenShareProfile } from "../../../packages/sdk-video/src";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** `target`/`systemAudio` заполняются только в нативном режиме (desktop). */
  onConfirm: (
    profile: ScreenShareProfile,
    target: NativeShareTarget | null,
    systemAudio: boolean,
  ) => void;
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
  const bridge = getNativeBridge();
  const [targets, setTargets] = useState<NativeShareTarget[]>([]);
  const [target, setTarget] = useState<NativeShareTarget | null>(null);
  const [systemAudio, setSystemAudio] = useState(true);
  const isMac =
    typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

  // Нативный режим: источники перечитываются на каждое открытие —
  // окна появляются и исчезают.
  useEffect(() => {
    if (!open || !bridge) return;
    let cancelled = false;
    bridge
      .listShareTargets()
      .then((list) => {
        if (cancelled) return;
        setTargets(list);
        setTarget(null); // сброс к "основному дисплею" на каждое открытие
      })
      .catch((err) => console.error("listShareTargets failed", err));
    return () => {
      cancelled = true;
    };
  }, [open, bridge]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("Share your screen")}</DialogTitle>
          <DialogDescription>
            {bridge
              ? t("Pick a quality profile and a source. Sharing runs through the native MateHub engine.")
              : t("Pick a quality profile. Your browser will ask which window or screen to share next.")}
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
                    <span className="text-sm font-semibold">{t(p.title)}</span>
                    <span className="font-mono text-[10px] text-muted-foreground">
                      {p.resolution}
                    </span>
                  </div>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    {t(p.description)}
                  </p>
                </div>
              </button>
            );
          })}
        </div>

        {bridge && (
          <div className="flex flex-col gap-1">
            <span className="text-xs font-medium text-muted-foreground">
              {t("Source")}
            </span>
            <div className="flex max-h-40 flex-col gap-1 overflow-y-auto">
              <button
                type="button"
                onClick={() => setTarget(null)}
                className={cn(
                  "flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-left text-xs transition",
                  target === null
                    ? "border-primary bg-primary/5"
                    : "border-border hover:bg-muted/50",
                )}
              >
                <Monitor className="h-3.5 w-3.5 shrink-0 text-primary" />
                {t("Primary display")}
              </button>
              {targets.map((s) => {
                const active =
                  target?.kind === s.kind && target?.id === s.id;
                const Icon = s.kind === "display" ? Monitor : AppWindow;
                return (
                  <button
                    key={`${s.kind}:${s.id}`}
                    type="button"
                    onClick={() => setTarget(s)}
                    className={cn(
                      "flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-left text-xs transition",
                      active
                        ? "border-primary bg-primary/5"
                        : "border-border hover:bg-muted/50",
                    )}
                  >
                    <Icon className="h-3.5 w-3.5 shrink-0 text-primary" />
                    <span className="truncate">{s.title}</span>
                  </button>
                );
              })}
            </div>
          </div>
        )}

        {bridge ? (
          <div className="flex items-center gap-2">
            <Checkbox
              id="system-audio"
              checked={systemAudio}
              onCheckedChange={(v) => setSystemAudio(v === true)}
            />
            <Label htmlFor="system-audio" className="text-xs font-normal">
              {t("Capture system audio")}
            </Label>
          </div>
        ) : (
          isMac && (
            <p className="rounded border border-amber-500/40 bg-amber-500/10 p-2 text-xs text-amber-600 dark:text-amber-400">
              {t("On macOS, game/system audio can only be captured from a browser tab. Sharing a full screen will be video-only.")}
            </p>
          )
        )}

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button
            onClick={() => {
              onConfirm(selected, target, systemAudio);
              onOpenChange(false);
            }}
          >
            {t("Start sharing")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
