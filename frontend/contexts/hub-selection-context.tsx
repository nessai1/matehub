import {
  createContext,
  useCallback,
  useContext,
  useSyncExternalStore,
  type ReactNode,
} from "react";

/**
 * Which text channel is shown in the chat workspace. Pure SPA state — the URL
 * never changes when the user navigates. Persisted to localStorage so the
 * selection survives reloads.
 *
 * Voice state lives in VideoCallProvider (mic/cam/participants/etc); we only
 * need to track the text channel here. The two selections are independent:
 * you can be in voice "office-watch" AND reading text "anime" at the same
 * time, and both render simultaneously.
 */

const STORAGE_KEY = "matehub:selected-text-channel";

function readStored(): string | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

let cache: string | null | undefined;
const listeners = new Set<() => void>();

function getSnapshot(): string | null {
  if (cache === undefined) cache = readStored();
  return cache ?? null;
}

function getServerSnapshot(): string | null {
  return null;
}

function subscribe(fn: () => void) {
  listeners.add(fn);
  const onStorage = (e: StorageEvent) => {
    if (e.key === STORAGE_KEY) {
      cache = e.newValue;
      fn();
    }
  };
  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(fn);
    window.removeEventListener("storage", onStorage);
  };
}

function write(id: string | null) {
  cache = id;
  if (typeof window !== "undefined") {
    try {
      if (id) window.localStorage.setItem(STORAGE_KEY, id);
      else window.localStorage.removeItem(STORAGE_KEY);
    } catch {
      /* ignore */
    }
  }
  for (const fn of listeners) fn();
}

// ── Public API ──────────────────────────────────────────────────────────────

interface HubSelectionValue {
  selectedTextChannelId: string | null;
  selectTextChannel: (id: string) => void;
}

const HubSelectionContext = createContext<HubSelectionValue | null>(null);

export function HubSelectionProvider({ children }: { children: ReactNode }) {
  const selectedTextChannelId = useSyncExternalStore(
    subscribe,
    getSnapshot,
    getServerSnapshot,
  );
  const selectTextChannel = useCallback((id: string) => {
    write(id);
  }, []);

  return (
    <HubSelectionContext.Provider value={{ selectedTextChannelId, selectTextChannel }}>
      {children}
    </HubSelectionContext.Provider>
  );
}

export function useHubSelection(): HubSelectionValue {
  const ctx = useContext(HubSelectionContext);
  if (!ctx) {
    throw new Error("useHubSelection must be used within <HubSelectionProvider>");
  }
  return ctx;
}
