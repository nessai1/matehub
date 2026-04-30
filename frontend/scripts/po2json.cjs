#!/usr/bin/env node
// Compiles every locales/<code>/LC_MESSAGES/messages.po into a Jed-compatible
// JSON at i18n/generated/<code>.json. Mirrors Apache Superset's
// scripts/po2json.sh but runs in pure Node — no python babel toolchain.

const fs = require("fs");
const path = require("path");
const gettext = require("gettext-parser");

const ROOT = path.join(__dirname, "..");
const LOCALES_DIR = path.join(ROOT, "locales");
const OUT_DIR = path.join(ROOT, "i18n", "generated");

if (!fs.existsSync(OUT_DIR)) fs.mkdirSync(OUT_DIR, { recursive: true });

const locales = fs
  .readdirSync(LOCALES_DIR)
  .filter((entry) =>
    fs.statSync(path.join(LOCALES_DIR, entry)).isDirectory(),
  );

for (const code of locales) {
  const poPath = path.join(LOCALES_DIR, code, "LC_MESSAGES", "messages.po");
  if (!fs.existsSync(poPath)) {
    console.warn(`[po2json] skip ${code}: no messages.po`);
    continue;
  }

  const buf = fs.readFileSync(poPath);
  const parsed = gettext.po.parse(buf);

  // Translate gettext-parser's tree to Jed1.x shape:
  //   { domain, locale_data: { matehub: { "": {meta}, msgid: [msgstr], ... } } }
  const messages = {
    "": {
      domain: "matehub",
      lang: code,
      plural_forms:
        parsed.headers["plural-forms"] || "nplurals=2; plural=(n != 1)",
    },
  };

  // gettext-parser puts entries under translations[ctx][msgid]; we ignore
  // contexts (default "") -- matehub doesn't use msgctxt.
  const ctx = parsed.translations[""] || {};
  for (const [msgid, entry] of Object.entries(ctx)) {
    if (msgid === "") continue; // header
    if (!entry.msgstr || entry.msgstr.every((s) => !s)) continue; // untranslated
    messages[msgid] = entry.msgstr;
  }

  const out = {
    domain: "matehub",
    locale_data: { matehub: messages },
  };

  const outPath = path.join(OUT_DIR, `${code}.json`);
  fs.writeFileSync(outPath, JSON.stringify(out, null, 2) + "\n");
  console.log(`[po2json] ${code}.po -> ${path.relative(ROOT, outPath)} (${
    Object.keys(messages).length - 1
  } strings)`);
}
