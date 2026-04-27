import { useState } from "react";
import {
  CheckIcon,
  Mic,
  MicOff,
  MoreVertical,
  Monitor,
  MonitorOff,
  PhoneOff,
  Video,
  VideoOff,
} from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useVideoCall } from "@/contexts/video-call-context";
import { cn } from "@/lib/utils";

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
  // No-camera/no-mic machines and revoked permissions surface as empty
  // device lists. Disable the corresponding toggle so the user can't trigger
  // an enableCamera/enableMic that's just going to fall over on getUserMedia.
  const { audioInputs, videoInputs } = useVideoCall();
  const cameraAvailable = videoInputs.length > 0;
  const micAvailable = audioInputs.length > 0;

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
        disabled={!micAvailable}
        onClick={onToggleMic}
        title={
          !micAvailable
            ? "No microphone detected"
            : isMicEnabled
              ? "Mute"
              : "Unmute"
        }
      >
        {isMicEnabled ? <Mic className="h-4 w-4" /> : <MicOff className="h-4 w-4" />}
      </ControlBtn>

      <ControlBtn
        active={isCamEnabled}
        disabled={!cameraAvailable}
        onClick={onToggleCamera}
        title={
          !cameraAvailable
            ? "No camera detected"
            : isCamEnabled
              ? "Turn off camera"
              : "Turn on camera"
        }
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

      <DeviceMenu />

      <ControlBtn danger onClick={onLeave} title="Leave call">
        <PhoneOff className="h-4 w-4" />
      </ControlBtn>
    </div>
  );
}

/**
 * Three-dots affordance for picking the active microphone and camera. Reads
 * the device lists straight off the call context — no prop drilling. Fires
 * setMicDevice / setCameraDevice which under the hood do `replaceTrack` on
 * the existing RTCRtpSender, so SSRC/SDP stay put and the SFU sees no blip.
 */
function DeviceMenu() {
  const {
    audioInputs,
    videoInputs,
    currentMicDeviceId,
    currentCameraDeviceId,
    setMicDevice,
    setCameraDevice,
    refreshDevices,
    isConnected,
  } = useVideoCall();

  const [hover, setHover] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  const onMic = async (id: string) => {
    if (id === currentMicDeviceId || busy) return;
    setBusy(`mic:${id}`);
    try {
      await setMicDevice(id);
    } catch (e) {
      console.warn("setMicDevice failed", e);
    } finally {
      setBusy(null);
    }
  };

  const onCam = async (id: string) => {
    if (id === currentCameraDeviceId || busy) return;
    setBusy(`cam:${id}`);
    try {
      await setCameraDevice(id);
    } catch (e) {
      console.warn("setCameraDevice failed", e);
    } finally {
      setBusy(null);
    }
  };

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        // Refresh on each open — labels fill in only after permission and
        // a USB plug between opens won't fire devicechange while the menu
        // is closed-and-mounted-but-idle.
        if (open) void refreshDevices();
      }}
    >
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          disabled={!isConnected}
          onMouseEnter={() => setHover(true)}
          onMouseLeave={() => setHover(false)}
          onFocus={() => setHover(true)}
          onBlur={() => setHover(false)}
          title="Audio & video devices"
          aria-label="Audio & video devices"
          className="grid h-[42px] w-[42px] place-items-center rounded-full transition-colors disabled:cursor-not-allowed disabled:opacity-40"
          style={{
            background: hover
              ? "rgba(255,255,255,0.22)"
              : "rgba(255,255,255,0.06)",
            color: hover ? "#fff" : "rgba(255,255,255,0.85)",
            border: "1px solid rgba(255,255,255,0.08)",
          }}
        >
          <MoreVertical className="h-4 w-4" />
        </button>
      </DropdownMenuTrigger>

      <DropdownMenuContent
        side="top"
        align="end"
        sideOffset={10}
        className="min-w-[260px] max-w-[320px]"
      >
        <DropdownMenuLabel className="flex items-center gap-2 text-xs">
          <Mic className="h-3.5 w-3.5" />
          Microphone
        </DropdownMenuLabel>
        {audioInputs.length === 0 ? (
          <div className="px-2 py-1.5 text-xs text-muted-foreground">
            No microphones found
          </div>
        ) : (
          audioInputs.map((d, i) => (
            <DeviceRow
              key={d.deviceId || `mic-${i}`}
              label={d.label || `Microphone ${i + 1}`}
              selected={d.deviceId === currentMicDeviceId}
              busy={busy === `mic:${d.deviceId}`}
              onClick={() => void onMic(d.deviceId)}
            />
          ))
        )}

        <DropdownMenuSeparator />

        <DropdownMenuLabel className="flex items-center gap-2 text-xs">
          <Video className="h-3.5 w-3.5" />
          Camera
        </DropdownMenuLabel>
        {videoInputs.length === 0 ? (
          <div className="px-2 py-1.5 text-xs text-muted-foreground">
            No cameras found
          </div>
        ) : (
          videoInputs.map((d, i) => (
            <DeviceRow
              key={d.deviceId || `cam-${i}`}
              label={d.label || `Camera ${i + 1}`}
              selected={d.deviceId === currentCameraDeviceId}
              busy={busy === `cam:${d.deviceId}`}
              onClick={() => void onCam(d.deviceId)}
            />
          ))
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function DeviceRow({
  label,
  selected,
  busy,
  onClick,
}: {
  label: string;
  selected: boolean;
  busy: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={(e) => {
        // Keep the menu open across a swap so the user can see the check
        // hop to the new device. Radix closes on item-click by default.
        e.preventDefault();
        onClick();
      }}
      disabled={busy}
      className={cn(
        "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm outline-none",
        "hover:bg-accent hover:text-accent-foreground",
        "focus:bg-accent focus:text-accent-foreground",
        "disabled:cursor-not-allowed disabled:opacity-60",
      )}
    >
      <span className="flex h-4 w-4 shrink-0 items-center justify-center">
        {selected ? <CheckIcon className="h-4 w-4" /> : null}
      </span>
      <span className="flex-1 truncate">{label}</span>
    </button>
  );
}

function ControlBtn({
  children,
  active,
  danger,
  disabled,
  onClick,
  title,
}: {
  children: React.ReactNode;
  active?: boolean;
  danger?: boolean;
  disabled?: boolean;
  onClick: () => void;
  title: string;
}) {
  const [hover, setHover] = useState(false);
  const showHover = hover && !disabled;

  const style: React.CSSProperties = danger
    ? {
        background: showHover ? "oklch(0.70 0.24 25)" : "oklch(0.62 0.22 25)",
        color: "#fff",
        border: "none",
      }
    : {
        background: showHover
          ? "rgba(255,255,255,0.22)"
          : active
            ? "rgba(255,255,255,0.14)"
            : "rgba(255,255,255,0.06)",
        color: active || showHover ? "#fff" : "rgba(255,255,255,0.85)",
        border: "1px solid rgba(255,255,255,0.08)",
      };

  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onFocus={() => setHover(true)}
      onBlur={() => setHover(false)}
      title={title}
      aria-label={title}
      className="grid h-[42px] w-[42px] place-items-center rounded-full transition-colors disabled:cursor-not-allowed disabled:opacity-40"
      style={style}
    >
      {children}
    </button>
  );
}
