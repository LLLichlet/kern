import { readFileSync } from "node:fs";
import path from "node:path";
import { defineConfig } from "astro/config";
import tailwindcss from "@tailwindcss/vite";

const kernGrammar = JSON.parse(
  readFileSync(
    path.resolve("..", "editors", "vscode", "syntaxes", "kern.tmLanguage.json"),
    "utf8"
  )
);

function normalizeBase(value) {
  if (!value || value === "/") {
    return "/";
  }

  const withLeading = value.startsWith("/") ? value : `/${value}`;
  return withLeading.endsWith("/") ? withLeading : `${withLeading}/`;
}

export default defineConfig({
  output: "static",
  site: process.env.SITE_URL,
  base: normalizeBase(process.env.SITE_BASE),
  markdown: {
    shikiConfig: {
      theme: "github-light",
      langs: [
        {
          ...kernGrammar,
          aliases: ["kern"]
        }
      ]
    }
  },
  vite: {
    plugins: [tailwindcss()]
  }
});
