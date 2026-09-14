import { dirname } from "path";
import { fileURLToPath } from "url";
import { createRequire } from "module";
import { FlatCompat } from "@eslint/eslintrc";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const require = createRequire(import.meta.url);

const compat = new FlatCompat({
  baseDirectory: __dirname,
  resolvePluginsRelativeTo: dirname(require.resolve("eslint-config-next/package.json")),
});

const eslintConfig = [
  ...compat.extends("next/core-web-vitals", "next/typescript"),
  {
    ignores: [".next/**", ".next.corrupt-*/**", "out/**"],
  },
  {
    // Existing debt is frozen here so the gate is immediately reproducible.
    // Release-critical paths below re-enable every rule as an error.
    rules: {
      "@next/next/no-assign-module-variable": "off",
      "@typescript-eslint/no-explicit-any": "off",
      "@typescript-eslint/no-require-imports": "off",
      "@typescript-eslint/no-unused-vars": "off",
      "prefer-const": "off",
      "react-hooks/exhaustive-deps": "off",
      "react/no-unescaped-entities": "off",
    },
  },
  {
    files: [
      "src/components/templates/**/*.{ts,tsx}",
      "src/lib/blocknote-markdown.ts",
      "src/lib/summary-language-preferences.ts",
      "src/services/templateService.ts",
      "src/types/summary-template.ts",
      "tests/lib/blocknote-markdown.test.ts",
      "tests/lib/summary-language-preferences.test.js",
      "tests/lib/template-*.test.{ts,tsx,js,mjs}",
    ],
    rules: {
      "@next/next/no-assign-module-variable": "error",
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-require-imports": "error",
      "@typescript-eslint/no-unused-vars": "error",
      "prefer-const": "error",
      "react-hooks/exhaustive-deps": "error",
      "react/no-unescaped-entities": "error",
    },
  },
];

export default eslintConfig;
