// Контракт моста между веб-UI и нативной оболочкой (Tauri desktop app).
//
// Оболочка инжектит `window.__MATEHUB_NATIVE__` (init-script) — веб-фронт
// НЕ знает про Tauri IPC, только про этот интерфейс. Скриншаринг в
// десктоп-приложении идёт через нативный пайплайн (scap → openh264 →
// str0m → SFU, см. docs/video/desktop-share.md), минуя getDisplayMedia
// и все браузерные потолки качества.

export interface NativeShareTarget {
  id: number;
  kind: "display" | "window";
  title: string;
}

export interface NativeShareConfig {
  /** Абсолютная HTTP-база video-сервиса (origin + /api/video). */
  baseUrl: string;
  hubId: string;
  channelId: string;
  /** Access JWT текущей сессии — нативный паблишер входит тем же юзером
   * (отдельный participant-слот через ?device=screen). */
  token: string;
  /** null → основной дисплей. */
  target: NativeShareTarget | null;
  fps: number;
  bitrateBps: number;
  /** Захватывать системный звук (macOS 13+/Windows; Linux — по возможности). */
  systemAudio: boolean;
}

export type NativeShareStatus =
  | { state: "connecting" }
  | { state: "publishing"; session_id: string }
  | { state: "stopped"; reason: string }
  | { state: "error"; message: string }
  | {
      state: "stats";
      frames_sent: number;
      frames_dropped: number;
      target_bitrate_bps: number;
    };

/** Конфиг нативного войса — origin активного хаба + `/api/video`. */
export interface NativeVoiceConfig {
  baseUrl: string;
  hubId: string;
  channelId: string;
  token: string;
}

export type NativeVoiceStatus =
  | { state: "connecting" }
  | { state: "connected"; participant_id: string }
  | { state: "participant_joined"; participant_id: string; user_id: string }
  | { state: "participant_left"; participant_id: string; user_id: string }
  | { state: "stopped"; reason: string }
  | { state: "error"; message: string };

export interface MateHubNativeBridge {
  /** Версия контракта — фронт может отсекать несовместимые оболочки. */
  version: number;
  // Screen share.
  listShareTargets(): Promise<NativeShareTarget[]>;
  startShare(config: NativeShareConfig): Promise<void>;
  stopShare(): Promise<void>;
  /** Подписка на статусы шаринга; возвращает unsubscribe. */
  onShareStatus(cb: (status: NativeShareStatus) => void): () => void;
  // Нативный войс (десктоп владеет войсом; webview в WebRTC-войс не входит).
  joinVoice(config: NativeVoiceConfig): Promise<void>;
  leaveVoice(): Promise<void>;
  setMute(muted: boolean): Promise<void>;
  setDeafen(deafened: boolean): Promise<void>;
  onVoiceStatus(cb: (status: NativeVoiceStatus) => void): () => void;
}

declare global {
  interface Window {
    __MATEHUB_NATIVE__?: MateHubNativeBridge;
  }
}

/** Текущая (и единственная) версия контракта. */
export const NATIVE_BRIDGE_VERSION = 1;

export function getNativeBridge(): MateHubNativeBridge | null {
  if (typeof window === "undefined") return null;
  const bridge = window.__MATEHUB_NATIVE__;
  if (!bridge || bridge.version !== NATIVE_BRIDGE_VERSION) return null;
  return bridge;
}
