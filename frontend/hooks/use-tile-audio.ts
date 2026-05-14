// Web Audio pipeline for one remote audio track on a tile.
//
// HTMLMediaElement.volume is clamped to [0, 1] by spec, so any "up to
// 200%" volume slider has to go through Web Audio: source → GainNode
// (gain.value ∈ [0, 2]) → destination.
//
// The <audio> element is still mounted (callers want a stable ref for
// autoplay-policy gestures and DOM-level diagnostics) but kept muted —
// playback flows through the GainNode. Without the mute step both
// pipelines would output at once and produce double audio.
//
// Pipeline rebuilds happen ONLY on track change. Volume / muted state
// updates twiddle `gain.gain.value` in place, so a slider drag is
// O(1) graph ops with zero re-allocation.

import { useEffect, useRef } from "react";
import { getAudioContext } from "@/lib/audio-context";

interface UseTileAudioOptions {
  /** Remote audio track, or null when none. Identity change rebuilds
   *  the graph; the same MediaStreamTrack rendered twice doesn't. */
  track: MediaStreamTrack | null;
  /** Multiplier 0.0–2.0. 1.0 = native. >1.0 amplifies, capped by the
   *  caller (the store clamps it). */
  volume: number;
  /** Hard mute (gain forced to 0). Separate from volume so the user can
   *  un-mute back to their previous slider value, not force-reset to 1. */
  muted: boolean;
}

/**
 * Returns a ref to attach to an `<audio>` element. The element itself
 * stays muted; audio playback comes through the Web Audio graph.
 *
 * Returning the ref instead of an `<audio>` JSX node keeps this hook
 * agnostic to the tile's markup — camera and screen tiles each render
 * the element in their own slot, this hook just wires the graph.
 */
export function useTileAudio({ track, volume, muted }: UseTileAudioOptions) {
  const audioRef = useRef<HTMLAudioElement>(null);
  const sourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const gainRef = useRef<GainNode | null>(null);

  // (Re)build the graph whenever the track identity changes. The
  // gain/source nodes are scoped to this effect so the cleanup path
  // disconnects them cleanly before a new track comes in.
  useEffect(() => {
    // Tear down previous graph (track change or unmount).
    sourceRef.current?.disconnect();
    gainRef.current?.disconnect();
    sourceRef.current = null;
    gainRef.current = null;

    const el = audioRef.current;
    if (!track) {
      if (el) el.srcObject = null;
      return;
    }

    const ctx = getAudioContext();
    const stream = new MediaStream([track]);

    // Mount the <audio> element with the stream — Safari needs an
    // <audio> in the DOM to keep the track "playing" for the source
    // node. Mute it so we don't double-output (graph → speakers AND
    // element → speakers).
    if (el) {
      el.srcObject = stream;
      el.muted = true;
      el.play().catch(() => {
        // Autoplay-restricted; once the user clicks something it'll
        // unblock both the element and the AudioContext.
      });
    }

    // Web Audio unavailable (SSR / ancient browsers). Fall back to the
    // unmuted element so audio still plays, just without the >100%
    // headroom — graceful degradation, not a hard failure.
    if (!ctx) {
      if (el) {
        el.muted = false;
        // Clamp into the native [0, 1] range — anything above just acts
        // as 1.0 here, which matches our UX promise that the user at
        // least HEARS the participant.
        el.volume = Math.min(1, Math.max(0, muted ? 0 : volume));
      }
      return;
    }

    let source: MediaStreamAudioSourceNode;
    try {
      source = ctx.createMediaStreamSource(stream);
    } catch (e) {
      console.warn("createMediaStreamSource failed", e);
      // Same fall-back path as no-ctx above.
      if (el) {
        el.muted = false;
        el.volume = Math.min(1, Math.max(0, muted ? 0 : volume));
      }
      return;
    }
    const gain = ctx.createGain();
    gain.gain.value = muted ? 0 : volume;
    source.connect(gain).connect(ctx.destination);
    sourceRef.current = source;
    gainRef.current = gain;

    return () => {
      source.disconnect();
      gain.disconnect();
    };
    // Intentionally omit volume / muted from deps — those don't need a
    // graph rebuild. The next effect picks them up via gain.value
    // mutation, an O(1) parameter set with no audible glitch.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [track]);

  // Apply volume / muted without rebuilding. `setValueAtTime` is the
  // sample-accurate way to twiddle a gain without click-pops on the
  // sliderr. `currentTime` is "right now" in audio-context clock terms.
  useEffect(() => {
    const target = muted ? 0 : volume;
    const gain = gainRef.current;
    if (gain) {
      // Schedule slightly in the future (0.01s) so the engine has time
      // to interpolate; with `setValueAtTime(target, 0)` Chrome
      // sometimes still rounds to a hard step.
      const ctx = getAudioContext();
      if (ctx) {
        try {
          gain.gain.setTargetAtTime(target, ctx.currentTime, 0.01);
          return;
        } catch {
          // fall through to direct assignment
        }
      }
      gain.gain.value = target;
      return;
    }
    // Fallback path: no graph (Web Audio unsupported / source creation
    // failed). The element is the only output knob we have.
    const el = audioRef.current;
    if (el) {
      el.muted = false;
      el.volume = Math.min(1, Math.max(0, target));
    }
  }, [volume, muted]);

  return audioRef;
}
