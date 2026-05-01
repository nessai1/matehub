/**
 * Hub-wide audio/video mute state per user.
 *
 * Live updates come from the hub's presence-WS `voice_mute` event. Cold
 * start is seeded from `members-full` so a freshly-loaded sidebar sees
 * everyone's current state without waiting for the next toggle.
 *
 * Keys are Snowflake `user_id`s as decimal strings (Sonyflake exceeds
 * MAX_SAFE_INTEGER — stringified end-to-end).
 */

type Listener = () => void;

export interface MuteState {
  audioMuted: boolean;
  videoMuted: boolean;
}

type Map_ = ReadonlyMap<string, MuteState>;

const listeners = new Set<Listener>();
let current: Map<string, MuteState> = new Map();
let snapshot: Map_ = current;

function notify() {
  snapshot = new Map(current);
  for (const fn of listeners) fn();
}

export function subscribeVoiceMute(fn: Listener): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function getVoiceMute(): Map_ {
  return snapshot;
}

export function getVoiceMuteServerSnapshot(): Map_ {
  return snapshot;
}

/** Replace the whole map in one shot — used for the /members-full bulk sync. */
export function seedVoiceMute(
  pairs: Iterable<[string, MuteState]>,
) {
  const next = new Map<string, MuteState>();
  for (const [uid, state] of pairs) {
    next.set(uid, { ...state });
  }
  if (mapsEqual(current, next)) return;
  current = next;
  notify();
}

/** Apply a single user/kind change. Default for missing user is fully muted. */
export function setVoiceMute(
  userId: string,
  kind: "audio" | "video",
  muted: boolean,
) {
  const prev = current.get(userId) ?? { audioMuted: true, videoMuted: true };
  const next: MuteState =
    kind === "audio"
      ? { ...prev, audioMuted: muted }
      : { ...prev, videoMuted: muted };
  if (
    prev.audioMuted === next.audioMuted &&
    prev.videoMuted === next.videoMuted &&
    current.has(userId)
  ) {
    return;
  }
  current.set(userId, next);
  notify();
}

/** Drop a user's mute entry — called on voice-leave so a stale state
 *  doesn't survive into their next session. */
export function clearVoiceMute(userId: string) {
  if (!current.has(userId)) return;
  current.delete(userId);
  notify();
}

function mapsEqual(a: Map<string, MuteState>, b: Map<string, MuteState>): boolean {
  if (a.size !== b.size) return false;
  for (const [k, v] of a) {
    const other = b.get(k);
    if (
      !other ||
      other.audioMuted !== v.audioMuted ||
      other.videoMuted !== v.videoMuted
    ) {
      return false;
    }
  }
  return true;
}
