import { useSyncExternalStore } from "react";
import {
  getVoiceOccupancy,
  getVoiceOccupancyServerSnapshot,
  subscribeVoiceOccupancy,
} from "@/lib/voice-occupancy-store";

/**
 * Returns the live `userId → channelId` map. Null-channel (left) rows are
 * simply absent from the map, so `occupancy.get(userId)` is `undefined` when
 * the user isn't in any voice channel.
 */
export function useVoiceOccupancy(): ReadonlyMap<number, number> {
  return useSyncExternalStore(
    subscribeVoiceOccupancy,
    getVoiceOccupancy,
    getVoiceOccupancyServerSnapshot,
  );
}
