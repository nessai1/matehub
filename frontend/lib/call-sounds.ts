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
  | "disable_desktop";

// Reuse one HTMLAudioElement per cue so rapid repeats don't leak DOM nodes.
const cache: Partial<Record<CallSound, HTMLAudioElement>> = {};

// Per-cue volume. The join/leave chimes are the ones that fire right when the
// mic is about to pick up ambient audio — a loud cue clips into the start of
// the conversation, so those get dialed way down.
const VOLUME: Record<CallSound, number> = {
  join_call: 0.15,
  leave_call: 0.2,
  show_desktop: 0.4,
  disable_desktop: 0.4,
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
