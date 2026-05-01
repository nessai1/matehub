import { useSyncExternalStore } from "react";
import {
  getVoiceMute,
  getVoiceMuteServerSnapshot,
  subscribeVoiceMute,
  type MuteState,
} from "@/lib/voice-mute-store";

/**
 * Live `userId → {audioMuted, videoMuted}` map for everyone in voice
 * across the hub. Seeded from `/members-full` and kept fresh by the
 * `voice_mute` presence-WS event.
 */
export function useVoiceMute(): ReadonlyMap<string, MuteState> {
  return useSyncExternalStore(
    subscribeVoiceMute,
    getVoiceMute,
    getVoiceMuteServerSnapshot,
  );
}
