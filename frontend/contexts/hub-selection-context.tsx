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
 */

const STORAGE_KEY = "matehub:selected-text-channel";

function readStored(): number | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === null) return null;
    const n = Number(raw);
    return Number.isFinite(n) ? n : null;
  } catch {
    return null;
  }
}

let cache: number | null | undefined;
const listeners = new Set<() => void>();

function getSnapshot(): number | null {
  if (cache === undefined) cache = readStored();
  return cache ?? null;
}

function getServerSnapshot(): number | null {
  return null;
}

function subscribe(fn: () => void) {
  listeners.add(fn);
  const onStorage = (e: StorageEvent) => {
    if (e.key === STORAGE_KEY) {
      const n = e.newValue === null ? null : Number(e.newValue);
      cache = n !== null && Number.isFinite(n) ? n : null;
      fn();
    }
  };
  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(fn);
    window.removeEventListener("storage", onStorage);
  };
}

function write(id: number | null) {
  cache = id;
  if (typeof window !== "undefined") {
    try {
      if (id !== null) window.localStorage.setItem(STORAGE_KEY, String(id));
      else window.localStorage.removeItem(STORAGE_KEY);
    } catch {
      /* ignore */
    }
  }
  for (const fn of listeners) fn();
}

// ── Public API ──────────────────────────────────────────────────────────────

interface HubSelectionValue {
  selectedTextChannelId: number | null;
  selectTextChannel: (id: number) => void;
}

const HubSelectionContext = createContext<HubSelectionValue | null>(null);

export function HubSelectionProvider({ children }: { children: ReactNode }) {
  const selectedTextChannelId = useSyncExternalStore(
    subscribe,
    getSnapshot,
    getServerSnapshot,
  );
  const selectTextChannel = useCallback((id: number) => {
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
