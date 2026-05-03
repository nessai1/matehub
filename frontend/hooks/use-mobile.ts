import * as React from "react"

const MOBILE_BREAKPOINT = 768

// useSyncExternalStore is the idiomatic way to subscribe a component to a
// browser API. The previous "setState in useEffect" version tripped React
// Compiler's set-state-in-effect rule and also flashed a wrong value on the
// first render.
const subscribe = (callback: () => void) => {
  const mql = window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT - 1}px)`)
  mql.addEventListener("change", callback)
  return () => mql.removeEventListener("change", callback)
}

const getSnapshot = () => window.innerWidth < MOBILE_BREAKPOINT
const getServerSnapshot = () => false

export function useIsMobile() {
  return React.useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
}
