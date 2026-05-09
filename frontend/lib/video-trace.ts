// Video-call debug surface toggle.
//
// On dev (`import.meta.env.DEV`) the in-call diagnostic panel
// (`CallDebugPanel`) is always mounted — we want maximum visibility while
// hacking on the video stack. On a deployed box the panel is hidden by
// default so end-users don't see the engineering UI, but support / on-call
// engineers need to be able to enable it from the browser console without
// rebuilding or redeploying.
//
// API for the console:
//
//     enableVideoTrace()   // turns the debug panel on, reloads the page
//     disableVideoTrace()  // turns it off, reloads
//     isVideoTraceEnabled()
//
// Implementation is a single localStorage flag — survives page reloads,
// scoped per-browser, no server round-trip.

const STORAGE_KEY = "matehub.video_trace_enabled";

export function isVideoTraceEnabled(): boolean {
  if (typeof localStorage === "undefined") return false;
  return localStorage.getItem(STORAGE_KEY) === "1";
}

export function enableVideoTrace(): void {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(STORAGE_KEY, "1");
  // The panel mounts at workspace render time, not on a state change, so
  // a reload is the simplest way to attach it to the live call without
  // wiring the flag through React state.
  if (typeof location !== "undefined") location.reload();
}

export function disableVideoTrace(): void {
  if (typeof localStorage === "undefined") return;
  localStorage.removeItem(STORAGE_KEY);
  if (typeof location !== "undefined") location.reload();
}

// Expose on `window` so support can type the function name in DevTools
// without an import line. This is a deliberate global — the alternative
// (a hidden URL param or a React-context toggle) would be discoverable
// only by people reading the source, which defeats the point.
declare global {
  interface Window {
    enableVideoTrace?: () => void;
    disableVideoTrace?: () => void;
    isVideoTraceEnabled?: () => boolean;
  }
}

if (typeof window !== "undefined") {
  window.enableVideoTrace = enableVideoTrace;
  window.disableVideoTrace = disableVideoTrace;
  window.isVideoTraceEnabled = isVideoTraceEnabled;
}
