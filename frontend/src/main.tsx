import { createRoot } from "react-dom/client";
import { RouterProvider } from "react-router";

import "@fontsource-variable/inter";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";

import "@/app/globals.css";
import { ThemeProvider } from "@/components/theme-provider";
import { AuthProvider } from "@/lib/auth";
import { router } from "./router";

const rootEl = document.getElementById("root")!;

// NB: no <StrictMode> wrapper. React dev-mode's double-invoke of effects is
// great for finding non-idempotent bugs, but this app spins up long-lived
// WebSockets (presence, chat) and a WebRTC PeerConnection on mount — the
// mount/unmount/mount cycle causes connect-close-connect flapping that shows
// up as "flicker" online status and visible voice-call join sounds during dev.
// The fix is to ship without StrictMode in dev; production doesn't double-run.
createRoot(rootEl).render(
  <ThemeProvider>
    <AuthProvider>
      <RouterProvider router={router} />
    </AuthProvider>
  </ThemeProvider>,
);
