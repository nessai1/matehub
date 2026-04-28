// Runtime config fetched from hub on bootstrap.
//
// Everything that varies per deployment (TURN credentials, app version)
// lives here. API URLs do NOT -- those are same-origin relative paths
// (`/api/hub`, `/api/chat`, `/api/video`) and don't depend on deployment.
//
// Loaded once in main.tsx before <App /> renders. After that, components
// read it synchronously via getConfig() / getTurnConfig().

export interface RuntimeConfig {
  turn: {
    url: string | null;
    username: string | null;
    credential: string | null;
  };
  app_version: string | null;
}

declare global {
  interface Window {
    __MATEHUB_CONFIG__?: RuntimeConfig;
  }
}

const FALLBACK: RuntimeConfig = {
  turn: { url: null, username: null, credential: null },
  app_version: null,
};

export async function loadConfig(): Promise<void> {
  try {
    const r = await fetch("/config.json", { credentials: "same-origin" });
    if (!r.ok) {
      console.warn(`/config.json returned ${r.status}, using empty config`);
      window.__MATEHUB_CONFIG__ = FALLBACK;
      return;
    }
    window.__MATEHUB_CONFIG__ = (await r.json()) as RuntimeConfig;
  } catch (e) {
    console.warn("/config.json fetch failed, using empty config", e);
    window.__MATEHUB_CONFIG__ = FALLBACK;
  }
}

export function getConfig(): RuntimeConfig {
  return window.__MATEHUB_CONFIG__ ?? FALLBACK;
}

export function getTurnConfig(): RTCIceServer | null {
  const { url, username, credential } = getConfig().turn;
  if (!url) return null;
  return {
    urls: url,
    username: username ?? undefined,
    credential: credential ?? undefined,
  };
}
