// Lazy-loads compiled translation JSONs (one per locale). The .json files
// are produced by scripts/po2json.cjs from .po sources at build time.
// Vite's `?url` + dynamic import keeps each locale in its own chunk so we
// don't ship Russian translations to a French user.

import { setLocaleData, type LocaleData } from "./translator";
import type { LocaleCode } from "./locales";

const cache = new Map<LocaleCode, LocaleData>();

export async function loadLocale(code: LocaleCode): Promise<void> {
  let pack = cache.get(code);
  if (!pack) {
    // Vite resolves these dynamic imports to per-locale chunks. The catch
    // returns an empty pack — translator falls back to the source msgid.
    try {
      const mod = await import(`./generated/${code}.json`);
      pack = (mod.default ?? mod) as LocaleData;
    } catch (e) {
      console.warn(`[i18n] failed to load locale ${code}, falling back`, e);
      return;
    }
    cache.set(code, pack);
  }
  setLocaleData(pack);
}
