import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { cn } from "@/lib/utils";

// ── Split/collapsed state persisted across reloads ──────────────────────────

const STORAGE_KEY = "matehub_workspace_split";

interface StoredState {
  split: number;
  collapsed: boolean;
}

function clampSplit(v: number) {
  return Math.min(0.85, Math.max(0.15, v));
}

function readStored(): StoredState {
  if (typeof window === "undefined") return { split: 0.5, collapsed: false };
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { split: 0.5, collapsed: false };
    const parsed = JSON.parse(raw) as Partial<StoredState>;
    return {
      split: typeof parsed.split === "number" ? clampSplit(parsed.split) : 0.5,
      collapsed: !!parsed.collapsed,
    };
  } catch {
    return { split: 0.5, collapsed: false };
  }
}

// Cached snapshot — useSyncExternalStore requires getSnapshot to be stable
// (return the same object when the underlying value hasn't changed), otherwise
// React warns about infinite loops. We invalidate it when we write.
let snapshotCache: StoredState | null = null;
function getSnapshot(): StoredState {
  if (snapshotCache) return snapshotCache;
  snapshotCache = readStored();
  return snapshotCache;
}

const EXTERNAL_LISTENERS = new Set<() => void>();
function subscribeExternal(fn: () => void) {
  EXTERNAL_LISTENERS.add(fn);
  const onStorage = (e: StorageEvent) => {
    if (e.key === STORAGE_KEY) {
      snapshotCache = null;
      fn();
    }
  };
  window.addEventListener("storage", onStorage);
  return () => {
    EXTERNAL_LISTENERS.delete(fn);
    window.removeEventListener("storage", onStorage);
  };
}

function writeStored(next: StoredState) {
  snapshotCache = next;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    /* ignore */
  }
  for (const fn of EXTERNAL_LISTENERS) fn();
}

const DEFAULT_SERVER_SNAPSHOT: StoredState = { split: 0.5, collapsed: false };
function getServerSnapshot() {
  return DEFAULT_SERVER_SNAPSHOT;
}

// ── Context exposed to children so headers can toggle collapse ──────────────

interface WorkspaceStackContextValue {
  collapsed: boolean;
  collapse: () => void;
  expand: () => void;
  toggleCollapsed: () => void;
}

const WorkspaceStackContext = createContext<WorkspaceStackContextValue | null>(
  null,
);

export function useWorkspaceStack(): WorkspaceStackContextValue {
  const ctx = useContext(WorkspaceStackContext);
  if (!ctx) {
    // Being outside the stack is a valid case — return a no-op instead of
    // throwing, so header controls still render but just don't do anything.
    return {
      collapsed: false,
      collapse: () => {},
      expand: () => {},
      toggleCollapsed: () => {},
    };
  }
  return ctx;
}

// ── Stack ───────────────────────────────────────────────────────────────────

interface WorkspaceStackProps {
  top: ReactNode | null;
  bottom: ReactNode | null;
  /** Rendered instead of `bottom` when the split is collapsed. */
  collapsedBottom?: ReactNode;
}

export function WorkspaceStack({
  top,
  bottom,
  collapsedBottom,
}: WorkspaceStackProps) {
  const stored = useSyncExternalStore(
    subscribeExternal,
    getSnapshot,
    getServerSnapshot,
  );
  const { split, collapsed } = stored;

  const setSplit = useCallback((next: number) => {
    writeStored({ split: clampSplit(next), collapsed: getSnapshot().collapsed });
  }, []);
  const setCollapsed = useCallback((next: boolean) => {
    writeStored({ split: getSnapshot().split, collapsed: next });
  }, []);

  // ── Drag-to-resize ────────────────────────────────────────────────────────

  const containerRef = useRef<HTMLDivElement>(null);
  const dragStart = useRef<{
    startY: number;
    startSplit: number;
    containerH: number;
  } | null>(null);

  const onHandleMouseDown = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (!containerRef.current) return;
      e.preventDefault();
      dragStart.current = {
        startY: e.clientY,
        startSplit: split,
        containerH: containerRef.current.getBoundingClientRect().height,
      };
      document.body.style.cursor = "ns-resize";
      document.body.style.userSelect = "none";
    },
    [split],
  );

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      const s = dragStart.current;
      if (!s) return;
      const dy = e.clientY - s.startY;
      const dSplit = dy / s.containerH;
      setSplit(s.startSplit + dSplit);
    };
    const onUp = () => {
      if (!dragStart.current) return;
      dragStart.current = null;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [setSplit]);

  const contextValue: WorkspaceStackContextValue = {
    collapsed,
    collapse: () => setCollapsed(true),
    expand: () => setCollapsed(false),
    toggleCollapsed: () => setCollapsed(!collapsed),
  };

  // ── Render cases ──────────────────────────────────────────────────────────

  if (!top && !bottom) {
    return (
      <div className="flex flex-1 items-center justify-center text-xs text-muted-foreground">
        Pick a channel on the left
      </div>
    );
  }

  return (
    <WorkspaceStackContext.Provider value={contextValue}>
      <div ref={containerRef} className="flex min-h-0 flex-1 flex-col">
        {top && bottom && !collapsed && (
          <>
            <div
              className="min-h-0 overflow-hidden"
              style={{ flex: `${split} 1 0%` }}
            >
              {top}
            </div>
            <ResizeHandle
              onMouseDown={onHandleMouseDown}
              onDoubleClick={() => setCollapsed(true)}
            />
            <div
              className="min-h-0 overflow-hidden"
              style={{ flex: `${1 - split} 1 0%` }}
            >
              {bottom}
            </div>
          </>
        )}
        {top && bottom && collapsed && (
          <>
            <div className="min-h-0 flex-1 overflow-hidden">{top}</div>
            {collapsedBottom}
          </>
        )}
        {top && !bottom && (
          <div className="min-h-0 flex-1 overflow-hidden">{top}</div>
        )}
        {!top && bottom && (
          <div className="min-h-0 flex-1 overflow-hidden">{bottom}</div>
        )}
      </div>
    </WorkspaceStackContext.Provider>
  );
}

// ── Resize handle ───────────────────────────────────────────────────────────

function ResizeHandle({
  onMouseDown,
  onDoubleClick,
}: {
  onMouseDown: (e: React.MouseEvent<HTMLDivElement>) => void;
  onDoubleClick: () => void;
}) {
  return (
    <div
      onMouseDown={onMouseDown}
      onDoubleClick={onDoubleClick}
      className={cn(
        "group/handle flex h-1.5 shrink-0 cursor-ns-resize items-center justify-center bg-background",
      )}
      role="separator"
      aria-orientation="horizontal"
      title="Drag to resize · double-click to collapse"
    >
      <div className="h-[3px] w-11 rounded-full bg-foreground/20 transition-colors group-hover/handle:bg-foreground/40" />
    </div>
  );
}
