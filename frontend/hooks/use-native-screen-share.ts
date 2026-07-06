import { useCallback, useEffect, useRef, useState } from "react";
import {
  getNativeBridge,
  type NativeShareStatus,
  type NativeShareTarget,
} from "@/lib/native-bridge";
import type { ScreenShareProfile } from "../../packages/sdk-video/src";

// Профили зеркалят PROFILE_CONFIG браузерного пути (screen-share.md §6):
// в нативном режиме тем же качеством публикуемся мимо getDisplayMedia.
const PROFILE_CONFIG: Record<
  ScreenShareProfile,
  { fps: number; bitrateBps: number }
> = {
  gaming: { fps: 60, bitrateBps: 6_000_000 },
  standard: { fps: 24, bitrateBps: 2_000_000 },
  detail: { fps: 5, bitrateBps: 1_000_000 },
};

export interface NativeShareParams {
  hubId: string;
  channelId: string;
  token: string;
}

interface NativeScreenShare {
  /** Приложение — нативная оболочка (мост доступен). */
  available: boolean;
  /** Идёт нативный шаринг. */
  sharing: boolean;
  status: NativeShareStatus | null;
  start(
    profile: ScreenShareProfile,
    target: NativeShareTarget | null,
    systemAudio: boolean,
    params: NativeShareParams,
  ): Promise<void>;
  stop(): Promise<void>;
}

export function useNativeScreenShare(): NativeScreenShare {
  const bridge = getNativeBridge();
  const [status, setStatus] = useState<NativeShareStatus | null>(null);
  const [sharing, setSharing] = useState(false);
  const unsubRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    if (!bridge) return;
    unsubRef.current = bridge.onShareStatus((s) => {
      setStatus(s);
      if (s.state === "publishing") setSharing(true);
      if (s.state === "stopped" || s.state === "error") setSharing(false);
    });
    return () => {
      unsubRef.current?.();
      unsubRef.current = null;
    };
  }, [bridge]);

  const start = useCallback(
    async (
      profile: ScreenShareProfile,
      target: NativeShareTarget | null,
      systemAudio: boolean,
      params: NativeShareParams,
    ) => {
      if (!bridge) throw new Error("native bridge unavailable");
      const cfg = PROFILE_CONFIG[profile] ?? PROFILE_CONFIG.gaming;
      // Абсолютный origin: нативный процесс не знает про относительный
      // /api/video, ему нужен полный адрес того же хаба.
      const baseUrl = `${window.location.origin}/api/video`;
      setSharing(true);
      await bridge.startShare({
        baseUrl,
        hubId: params.hubId,
        channelId: params.channelId,
        token: params.token,
        target,
        fps: cfg.fps,
        bitrateBps: cfg.bitrateBps,
        systemAudio,
      });
    },
    [bridge],
  );

  const stop = useCallback(async () => {
    if (!bridge) return;
    await bridge.stopShare();
    setSharing(false);
  }, [bridge]);

  return { available: !!bridge, sharing, status, start, stop };
}
