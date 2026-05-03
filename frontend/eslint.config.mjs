import js from "@eslint/js";
import tsPlugin from "@typescript-eslint/eslint-plugin";
import tsParser from "@typescript-eslint/parser";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";

export default [
  {
    ignores: [
      "dist/**",
      "node_modules/**",
      ".next/**",
      "next-env.d.ts",
      "stage1.txt",
    ],
  },
  js.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      parser: tsParser,
      parserOptions: { ecmaVersion: 2022, sourceType: "module" },
      globals: { ...globals.browser, ...globals.node },
    },
    plugins: {
      "@typescript-eslint": tsPlugin,
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      ...tsPlugin.configs.recommended.rules,
      ...reactHooks.configs.recommended.rules,
      // TS already enforces this and knows DOM/lib globals (React, RTCIceServer, …);
      // duplicating it in eslint just produces noise on JSX-runtime imports.
      "no-undef": "off",
      "@typescript-eslint/no-unused-vars": [
        "warn",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/no-explicit-any": "warn",
      // Hint at React Compiler readiness, but don't block CI on it: the
      // project doesn't have the compiler enabled yet, so manual useCallback
      // is still load-bearing for referential stability.
      "react-hooks/preserve-manual-memoization": "warn",
      "react-refresh/only-export-components": "off",
    },
  },
  {
    files: ["**/*.cjs", "scripts/**/*.{js,cjs}"],
    languageOptions: {
      sourceType: "commonjs",
      globals: { ...globals.node },
    },
  },
];
