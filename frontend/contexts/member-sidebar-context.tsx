// State for the right-hand member sidebar. Lives at hub layout level so the
// chat workspace headers can toggle it without prop-drilling, and so the
// open/collapsed state survives channel switches.
//
// Auto-collapse: below COLLAPSE_BREAKPOINT (1024px) the panel collapses; at
// or above it, expands. Media query wins on every resize event — manual
// toggles are always respected for the *current* viewport, but crossing the
// breakpoint resets the state to whatever the new viewport calls for. This
// keeps things predictable: drag the window narrow, sidebar shrinks; drag it
// back wide, sidebar reappears.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

const COLLAPSE_BREAKPOINT = 1024;

interface Ctx {
  open: boolean;
  toggle: () => void;
  setOpen: (open: boolean) => void;
}

const MemberSidebarContext = createContext<Ctx | null>(null);

export function MemberSidebarProvider({ children }: { children: React.ReactNode }) {
  const [open, setOpenState] = useState<boolean>(() => {
    if (typeof window !== "undefined") {
      return window.innerWidth >= COLLAPSE_BREAKPOINT;
    }
    return true;
  });

  const setOpen = useCallback((next: boolean) => {
    setOpenState(next);
  }, []);

  const toggle = useCallback(() => setOpenState((v) => !v), []);

  // Re-evaluate on every breakpoint crossing. Using `change` events (not
  // resize) means we only react when the viewport class actually flips, so
  // mid-drag jitter won't spam state updates.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const mql = window.matchMedia(`(min-width: ${COLLAPSE_BREAKPOINT}px)`);
    const apply = () => setOpenState(mql.matches);
    apply();
    mql.addEventListener("change", apply);
    return () => mql.removeEventListener("change", apply);
  }, []);

  const value = useMemo<Ctx>(() => ({ open, toggle, setOpen }), [open, toggle, setOpen]);

  return (
    <MemberSidebarContext.Provider value={value}>
      {children}
    </MemberSidebarContext.Provider>
  );
}

export function useMemberSidebar(): Ctx {
  const ctx = useContext(MemberSidebarContext);
  if (!ctx) {
    throw new Error("useMemberSidebar must be used within MemberSidebarProvider");
  }
  return ctx;
}
