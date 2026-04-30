// Catalogue of supported locales.

export const LOCALES = [
  { code: "en", label: "English", nativeLabel: "English" },
  { code: "ru", label: "Russian", nativeLabel: "Русский" },
  { code: "de", label: "German", nativeLabel: "Deutsch" },
  { code: "fr", label: "French", nativeLabel: "Français" },
  { code: "es", label: "Spanish", nativeLabel: "Español" },
  { code: "zh", label: "Chinese", nativeLabel: "中文" },
  { code: "ja", label: "Japanese", nativeLabel: "日本語" },
] as const;

export type LocaleCode = (typeof LOCALES)[number]["code"];

export const DEFAULT_LOCALE: LocaleCode = "en";
export const STORAGE_KEY = "matehub_locale";

export function detectInitialLocale(): LocaleCode {
  const stored = typeof window !== "undefined" ? window.localStorage.getItem(STORAGE_KEY) : null;
  if (stored && LOCALES.some((l) => l.code === stored)) {
    return stored as LocaleCode;
  }
  if (typeof navigator !== "undefined") {
    const browser = navigator.language.split("-")[0];
    const match = LOCALES.find((l) => l.code === browser);
    if (match) return match.code;
  }
  return DEFAULT_LOCALE;
}
