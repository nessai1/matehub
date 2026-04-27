/**
 * Tiny audio-cue player for voice-call UX events.
 *
 * The browser only lets us play audio after a user gesture; these cues fire
 * from click handlers (Join button, Start/Stop Sharing, Leave) so that
 * requirement is already met organically. If it's ever not met (e.g. programmatic
 * call on mount) the `play()` promise rejects silently — not fatal.
 *
 * Sources: generated via ElevenLabs SFX, converted to mono 48kHz Opus-in-OGG
 * by ffmpeg (~15KB each), committed under `frontend/public/sounds/`.
 */

export type CallSound =
  | "join_call"
  | "leave_call"
  | "show_desktop"
  | "disable_desktop"
  | "incoming_call";

// Reuse one HTMLAudioElement per cue so rapid repeats don't leak DOM nodes.
const cache: Partial<Record<CallSound, HTMLAudioElement>> = {};

// Per-cue volume. The join/leave chimes are the ones that fire right when the
// mic is about to pick up ambient audio — a loud cue clips into the start of
// the conversation, so those get dialed way down. The incoming-call ringtone
// loops while the user decides — louder than the chimes, but still polite.
const VOLUME: Record<CallSound, number> = {
  join_call: 0.15,
  leave_call: 0.2,
  show_desktop: 0.4,
  disable_desktop: 0.4,
  incoming_call: 0.45,
};

function load(name: CallSound): HTMLAudioElement | null {
  if (typeof window === "undefined") return null;
  const hit = cache[name];
  if (hit) return hit;
  const el = new Audio(`/sounds/${name}.ogg`);
  el.preload = "auto";
  el.volume = VOLUME[name];
  cache[name] = el;
  return el;
}

export function playCallSound(name: CallSound) {
  const el = load(name);
  if (!el) return;
  // Rewind so back-to-back events (e.g. two participants joining) retrigger.
  try {
    el.currentTime = 0;
  } catch {
    // currentTime can throw before metadata has loaded — ignore.
  }
  void el.play().catch(() => {
    // Autoplay restriction; the next user click will let it through.
  });
}

/**
 * Start looping a sound until the returned `stop()` is called. Used for
 * sustained UI states like an incoming-call dialog. Different cache slot from
 * the one-shot version above — otherwise stopping the loop would also kill any
 * playCallSound() call that grabbed the same element.
 */
const loopCache: Partial<Record<CallSound, HTMLAudioElement>> = {};

export function startCallSoundLoop(name: CallSound): () => void {
  if (typeof window === "undefined") return () => {};
  let el = loopCache[name];
  if (!el) {
    el = new Audio(`/sounds/${name}.ogg`);
    el.preload = "auto";
    el.loop = true;
    el.volume = VOLUME[name];
    loopCache[name] = el;
  }
  try {
    el.currentTime = 0;
  } catch {
    /* metadata not loaded yet */
  }
  void el.play().catch(() => {
    // Autoplay-restricted; the user click that opened the dialog usually
    // satisfies the gesture requirement, but on cold-start it might not.
  });

  const ref = el;
  return () => {
    ref.pause();
    try {
      ref.currentTime = 0;
    } catch {
      /* ignore */
    }
  };
}
