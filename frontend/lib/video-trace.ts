// Video-call debug surface toggle.
//
// On dev (`import.meta.env.DEV`) the in-call diagnostic panel
// (`CallDebugPanel`) is always mounted — we want maximum visibility while
// hacking on the video stack. On a deployed box the panel is hidden by
// default so end-users don't see the engineering UI; support / on-call
// engineers flip a localStorage flag via the browser console:
//
//     enableVideoTrace()   // panel appears in any active call
//     disableVideoTrace()  // panel disappears
//     isVideoTraceEnabled()
//
// The functions never reload the page. The primary use case is
// debugging a *live* problem call — `location.reload()` would tear down
// the WebRTC session, killing the very call we're trying to inspect.
// Instead they fire a custom event that VideoWorkspace listens to, so
// React mounts/unmounts CallDebugPanel without losing the
// RTCPeerConnection.
//
// Trust note: setting the flag is unconditional — anyone with
// `window.enableVideoTrace` can flip it. The privileged check happens at
// *render time* in VideoWorkspace, gated on `usePermissions().is_admin`.
// That way an XSS payload in a regular user's session can set the flag
// all it wants and the panel still won't render — the panel is the
// thing that exposes diagnostics, not the flag.

const STORAGE_KEY = "matehub.video_trace_enabled";
/** Same-tab change event. Storage events only fire in *other* tabs, so
 *  we dispatch this one ourselves for the tab that toggled. */
const CHANGE_EVENT = "matehub:video-trace-changed";

export function isVideoTraceEnabled(): boolean {
  if (typeof localStorage === "undefined") return false;
  return localStorage.getItem(STORAGE_KEY) === "1";
}

export function enableVideoTrace(): void {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(STORAGE_KEY, "1");
  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event(CHANGE_EVENT));
  }
  // Honest output: the flag is set even for non-privileged users; the
  // panel itself only renders if the operator's session passes the
  // admin check. Without this hint the user sees nothing happen and
  // assumes the function is broken.
  console.log(
    "[video-trace] flag set. The debug panel will appear in active calls " +
      "if your session has admin privileges.",
  );
}

export function disableVideoTrace(): void {
  if (typeof localStorage === "undefined") return;
  localStorage.removeItem(STORAGE_KEY);
  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event(CHANGE_EVENT));
  }
  console.log("[video-trace] flag cleared.");
}

/** Subscribe to enable/disable changes. Returns an unsubscribe fn.
 *  Listens to both the same-tab CHANGE_EVENT and the cross-tab
 *  `storage` event so a toggle in one tab updates panels in others. */
export function onVideoTraceChange(handler: () => void): () => void {
  if (typeof window === "undefined") return () => {};
  const sameTabHandler = () => handler();
  const storageHandler = (e: StorageEvent) => {
    if (e.key === STORAGE_KEY) handler();
  };
  window.addEventListener(CHANGE_EVENT, sameTabHandler);
  window.addEventListener("storage", storageHandler);
  return () => {
    window.removeEventListener(CHANGE_EVENT, sameTabHandler);
    window.removeEventListener("storage", storageHandler);
  };
}

// Expose on `window` so support can type the function name in DevTools
// without an import line. The actual privilege check happens at the
// render gate in VideoWorkspace — see file header.
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
