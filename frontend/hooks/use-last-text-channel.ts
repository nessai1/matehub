"use client";

import { useCallback, useState } from "react";

const STORAGE_KEY = "matehub:last-text-channel";

function readStored(): string | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage.getItem(STORAGE_KEY);
  } catch {
    // localStorage can throw in private browsing — ignore, start fresh.
    return null;
  }
}

/**
 * Remember the most recently visited text channel so that, when the user jumps
 * into a voice channel, we can put that text channel below the call tile —
 * same UX as Discord's channel switcher keeping chat context while you talk.
 *
 * Initialised via lazy state init so hydration reads localStorage synchronously
 * on first render (no flicker, and no setState-in-effect ESLint warning).
 */
export function useLastTextChannel() {
  const [lastTextChannelId, setLastTextChannelIdState] = useState<string | null>(
    readStored,
  );

  const setLastTextChannelId = useCallback((id: string | null) => {
    setLastTextChannelIdState(id);
    if (typeof window === "undefined") return;
    try {
      if (id) window.localStorage.setItem(STORAGE_KEY, id);
      else window.localStorage.removeItem(STORAGE_KEY);
    } catch {
      // Quota / access errors — non-fatal.
    }
  }, []);

  return { lastTextChannelId, setLastTextChannelId };
}
