// Per-participant volume store with O(1)-keyed subscriptions.
//
// Why this lives outside React state:
//
// The naive implementation kept a `Map<userId, number>` in the
// `VideoCallContext` value. Updating it (slider drag) produced a new Map
// reference, which made React re-render *every* consumer of
// `useVideoCall()` — that's the controls bar, the workspace, and every
// camera tile. On a 20-seat call a slider drag (50 events/sec) means
// 20 × 50 = 1000 tile re-renders per second of mouse movement. Frame
// budget eaten on layout work, none of it visible to the user dragging
// the slider on one tile.
//
// `useSyncExternalStore` flips the model: each subscriber declares which
// key it cares about, and only that key's listeners fire on a mutation.
// Dragging the slider on Alice's tile triggers exactly one Alice-tile
// re-render. The rest of the UI stays put.
//
// The store is intentionally a plain class, not a `useReducer`/Redux
// shape. React state machinery doesn't buy us anything here: there's
// no time-travel, no devtools-replay, no concurrent-mode tearing
// concerns (the source of truth is the audio element's `.volume`
// property, mutating mid-render is fine).

import { useSyncExternalStore } from "react";

/** Volume override per participant, scoped to one user id. 1.0 = default. */
type Listener = () => void;

export class ParticipantVolumeStore {
  // 1.0 is "no override". Entries at 1.0 are deleted from the map so
  // the store stays small and `getSnapshot` returns a stable default
  // for participants who never had their slider touched.
  private volumes = new Map<string, number>();
  // Per-key listener sets. A subscriber only wakes when its key
  // changes, which is the whole point of the rework.
  private listeners = new Map<string, Set<Listener>>();

  /** Read the current effective volume. 1.0 if no override is set. */
  get(userId: string): number {
    return this.volumes.get(userId) ?? 1;
  }

  /** Replace the volume for one participant. Clamps to [0, 1].
   *  Setting back to 1.0 (or within an epsilon of it) removes the entry —
   *  see why in the class doc. No-ops if the value didn't actually
   *  change, so we don't notify subscribers for nothing. */
  set(userId: string, volume: number): void {
    const clamped = Math.max(0, Math.min(1, volume));
    const isDefault = Math.abs(clamped - 1) < 0.001;
    const current = this.volumes.get(userId);

    if (isDefault) {
      if (current === undefined) return; // already at default
      this.volumes.delete(userId);
    } else {
      if (current === clamped) return; // identical
      this.volumes.set(userId, clamped);
    }
    this.notify(userId);
  }

  /** Drop every override. Used on `leaveVoice` — cross-call carryover
   *  would surprise the user ("why is Alice quiet in this completely
   *  different call?"). */
  reset(): void {
    if (this.volumes.size === 0) return;
    const keys = [...this.volumes.keys()];
    this.volumes.clear();
    for (const k of keys) this.notify(k);
  }

  /** useSyncExternalStore subscribe contract: register a listener
   *  scoped to one key, return a cleanup. */
  subscribe(userId: string, listener: Listener): () => void {
    let bucket = this.listeners.get(userId);
    if (!bucket) {
      bucket = new Set();
      this.listeners.set(userId, bucket);
    }
    bucket.add(listener);
    return () => {
      const current = this.listeners.get(userId);
      if (!current) return;
      current.delete(listener);
      if (current.size === 0) this.listeners.delete(userId);
    };
  }

  private notify(userId: string): void {
    this.listeners.get(userId)?.forEach((l) => l());
  }
}

/** React hook: subscribe to one participant's volume.
 *  Re-renders the caller ONLY when that participant's value changes. */
export function useParticipantVolume(
  store: ParticipantVolumeStore,
  userId: string,
): number {
  return useSyncExternalStore(
    (listener) => store.subscribe(userId, listener),
    () => store.get(userId),
    // SSR snapshot — `volume` is meaningless on the server, default to 1.
    () => 1,
  );
}
