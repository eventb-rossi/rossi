const js = require("@eslint/js");
const tsPlugin = require("@typescript-eslint/eslint-plugin");
const tsParser = require("@typescript-eslint/parser");

module.exports = [
  {
    ignores: ["out/**"],
  },
  js.configs.recommended,
  ...tsPlugin.configs["flat/recommended"],
  {
    files: ["src/**/*.ts"],
    languageOptions: {
      ecmaVersion: 2020,
      sourceType: "module",
      parser: tsParser,
      globals: {
        Buffer: "readonly",
        console: "readonly",
        process: "readonly",
      },
    },
  },
];
