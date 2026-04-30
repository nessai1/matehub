// Mirrors Apache Superset's Translator (superset-frontend/packages/
// superset-ui-core/src/translation/Translator.ts) so the pattern is
// familiar to anyone who's worked on Superset i18n. Difference: we run
// the pipeline in pure Node (gettext-parser at build time + jed at
// runtime), no Python babel.

// @ts-expect-error - jed has no types of its own
import UntypedJed from "jed";

export type LocaleData = {
  domain: string;
  locale_data: {
    matehub: {
      "": {
        domain: string;
        lang: string;
        plural_forms: string;
      };
      [msgid: string]: string[] | { domain: string; lang: string; plural_forms: string };
    };
  };
};

const FALLBACK_PACK: LocaleData = {
  domain: "matehub",
  locale_data: {
    matehub: {
      "": {
        domain: "matehub",
        lang: "en",
        plural_forms: "nplurals=2; plural=(n != 1)",
      },
    },
  },
};

interface JedInstance {
  translate(key: string): { fetch(...args: unknown[]): string };
}

let jedInstance: JedInstance = new UntypedJed(FALLBACK_PACK);
let currentLocale = "en";

export function setLocaleData(pack: LocaleData) {
  jedInstance = new UntypedJed(pack);
  const meta = pack.locale_data?.matehub?.[""];
  if (meta && typeof meta === "object" && "lang" in meta) {
    currentLocale = meta.lang;
  }
}

export function getCurrentLocale(): string {
  return currentLocale;
}

/** Translate a source string. Same name and signature as Superset's `t`. */
export function t(input: string, ...args: unknown[]): string {
  try {
    return jedInstance.translate(input).fetch(...args);
  } catch {
    return input;
  }
}
