// Public API for i18n. Components import { t } from "@/i18n" and that's it.

export { t, getCurrentLocale, setLocaleData } from "./translator";
export type { LocaleData } from "./translator";
export {
  LOCALES,
  DEFAULT_LOCALE,
  STORAGE_KEY,
  detectInitialLocale,
} from "./locales";
export type { LocaleCode } from "./locales";
export { LocaleProvider, useLocale } from "./react";
export { loadLocale } from "./loader";
