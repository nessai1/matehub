// React glue: a tiny context that re-renders consumers when the locale
// flips. We don't manage translation state inside React (the Jed instance
// is module-global), but components need a re-render trigger when the
// user picks a new language.

import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
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
  /** Bumps on every locale change — useful as a key to force re-render. */
  generation: number;
}

const LocaleContext = createContext<LocaleContextValue>({
  locale: DEFAULT_LOCALE,
  setLocale: async () => {},
  generation: 0,
});

export function LocaleProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<LocaleCode>(detectInitialLocale);
  const [generation, setGeneration] = useState(0);

  // No first-load effect needed: main.tsx hydrates the translator with the
  // initial locale before this provider mounts. setLocale below reloads the
  // page on switch, which re-runs that bootstrap.

  const setLocale = useCallback(async (code: LocaleCode) => {
    // Persist the choice and reload — same approach Superset uses. Avoids
    // the "where do I trigger a re-render of every t() call site" problem,
    // and the page bootstraps with the chosen pack already loaded.
    window.localStorage.setItem(STORAGE_KEY, code);
    window.location.reload();
  }, []);

  const value = useMemo(
    () => ({ locale, setLocale, generation }),
    [locale, setLocale, generation],
  );

  return <LocaleContext.Provider value={value}>{children}</LocaleContext.Provider>;
}

export function useLocale() {
  return useContext(LocaleContext);
}
