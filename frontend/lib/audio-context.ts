// Single shared AudioContext for all remote-audio pipelines in a tab.
//
// Why one AudioContext and not one-per-tile:
//
// - Browsers cap the number of concurrent AudioContexts (Chrome around
//   6, Safari more strict). A 30-participant call would blow through
//   that limit on the second screen-share track.
// - Each AudioContext is a heavyweight resource: an audio worker thread,
//   a sample-rate converter, and an output device handle. Sharing one
//   amortises all of that.
// - GainNodes / MediaStreamAudioSourceNodes inside the same context are
//   cheap (graph nodes, not OS resources). Spinning one up per remote
//   track is fine.
//
// Why lazy + gated on user gesture:
//
// Both Chrome and Safari ship the AudioContext in `state: "suspended"`
// until a user gesture (click/tap/keypress) resumes it. The voice-call
// UX has a natural gesture — clicking Join — that we piggyback on.
// `getAudioContext()` is called from inside that click handler chain
// (joinVoice → … → useTileAudio's first run), and we call `.resume()`
// from the same call. If for some reason no gesture has fired yet,
// `.resume()` is a no-op promise reject and the call simply plays no
// remote audio until the next gesture — same failure mode <audio>
// elements have with autoplay-restricted media.

let ctx: AudioContext | null = null;

type AudioContextConstructor = new () => AudioContext;

function ctor(): AudioContextConstructor | null {
  if (typeof window === "undefined") return null;
  // window.AudioContext is the standard; window.webkitAudioContext is
  // the Safari < 14 prefix. Most production code can drop the prefix
  // now (Safari 14+), but the cost of the fallback is one `||`.
  const w = window as unknown as {
    AudioContext?: AudioContextConstructor;
    webkitAudioContext?: AudioContextConstructor;
  };
  return w.AudioContext ?? w.webkitAudioContext ?? null;
}

/**
 * Get (and lazily create) the shared AudioContext.
 *
 * On the first call we instantiate the context and immediately try to
 * resume it — that's a no-op if the current execution stack came from a
 * user gesture, and rejects silently otherwise.
 *
 * Returns null in SSR or browsers without the Web Audio API.
 */
export function getAudioContext(): AudioContext | null {
  if (ctx) {
    // Suspended → resume. This handles tab-throttling: Chrome auto-
    // suspends AudioContexts for backgrounded tabs to save battery;
    // when the tab comes back to focus, the first useTileAudio render
    // re-triggers a resume.
    if (ctx.state === "suspended") {
      void ctx.resume().catch(() => {
        // Not in a gesture window. We'll try again on the next call.
      });
    }
    return ctx;
  }
  const Ctor = ctor();
  if (!Ctor) return null;
  try {
    ctx = new Ctor();
    void ctx.resume().catch(() => {
      // see above
    });
    return ctx;
  } catch (e) {
    console.warn("AudioContext init failed", e);
    return null;
  }
}
