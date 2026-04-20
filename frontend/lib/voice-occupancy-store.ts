/**
 * Global "who is in which voice channel right now" map.
 *
 * Two sources feed the store:
 *   1. `/v1/hubs/<id>/members-full` — initial snapshot + poll-refresh.
 *   2. Presence WS `voice_occupancy` events — push updates from the hub
 *      service (originating in video service NATS publishes).
 *
 * Exposed via `useSyncExternalStore` so every consumer sees the same Map
 * without Context-prop-drill.
 */

type Listener = () => void;

type Occupancy = ReadonlyMap<string, string>;

const listeners = new Set<Listener>();
let current: Map<string, string> = new Map();
let snapshot: Occupancy = current;

function notify() {
  // Fresh reference — useSyncExternalStore treats equal references as "no
  // change", so we always hand out a new readonly view on write.
  snapshot = new Map(current);
  for (const fn of listeners) fn();
}

export function subscribeVoiceOccupancy(fn: Listener): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function getVoiceOccupancy(): Occupancy {
  return snapshot;
}

export function getVoiceOccupancyServerSnapshot(): Occupancy {
  return snapshot;
}

/** Replace the whole map in one shot — used for the /members-full bulk sync. */
export function seedVoiceOccupancy(pairs: Iterable<[string, string | null]>) {
  const next = new Map<string, string>();
  for (const [uid, cid] of pairs) {
    if (cid) next.set(uid, cid);
  }
  // Avoid notify-on-equal: comparing two maps shallowly is cheap.
  if (mapsEqual(current, next)) return;
  current = next;
  notify();
}

/** Apply a single user's change. `channelId === null` means "left voice". */
export function setVoiceOccupancy(userId: string, channelId: string | null) {
  if (channelId === null) {
    if (!current.has(userId)) return;
    current.delete(userId);
  } else {
    if (current.get(userId) === channelId) return;
    current.set(userId, channelId);
  }
  notify();
}

function mapsEqual(a: Map<string, string>, b: Map<string, string>): boolean {
  if (a.size !== b.size) return false;
  for (const [k, v] of a) {
    if (b.get(k) !== v) return false;
  }
  return true;
}
