// React glue: a tiny context that re-renders consumers when the locale
// flips. We don't manage translation state inside React (the Jed instance
// is module-global), but components need a re-render trigger when the
// user picks a new language.

import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  type ReactNode,
} from "react";
import {
  DEFAULT_LOCALE,
  STORAGE_KEY,
  detectInitialLocale,
  type LocaleCode,
} from "./locales";

interface LocaleContextValue {
  locale: LocaleCode;
  setLocale: (code: LocaleCode) => Promise<void>;
}

const LocaleContext = createContext<LocaleContextValue>({
  locale: DEFAULT_LOCALE,
  setLocale: async () => {},
});

export function LocaleProvider({ children }: { children: ReactNode }) {
  // Locale is fixed for the provider's lifetime: setLocale reloads the page
  // and bootstrap re-detects from localStorage on the next mount, so there's
  // no in-place state to track.
  const locale = useMemo(() => detectInitialLocale(), []);

  const setLocale = useCallback(async (code: LocaleCode) => {
    // Persist the choice and reload — same approach Superset uses. Avoids
    // the "where do I trigger a re-render of every t() call site" problem,
    // and the page bootstraps with the chosen pack already loaded.
    window.localStorage.setItem(STORAGE_KEY, code);
    window.location.reload();
  }, []);

  const value = useMemo(() => ({ locale, setLocale }), [locale, setLocale]);

  return <LocaleContext.Provider value={value}>{children}</LocaleContext.Provider>;
}

export function useLocale() {
  return useContext(LocaleContext);
}
