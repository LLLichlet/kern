import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";

const distRoot = path.resolve("dist");

function normalizeBase(base) {
  if (!base || base === "/") {
    return "/";
  }

  const withLeading = base.startsWith("/") ? base : `/${base}`;
  return withLeading.endsWith("/") ? withLeading : `${withLeading}/`;
}

function isAllowedRootRelative(url, base) {
  if (!url.startsWith("/")) {
    return true;
  }

  if (base === "/") {
    return true;
  }

  return url === base.slice(0, -1) || url.startsWith(base);
}

function collectHtmlFiles(root) {
  const results = [];

  for (const entry of readdirSync(root)) {
    const fullPath = path.join(root, entry);
    const stat = statSync(fullPath);
    if (stat.isDirectory()) {
      results.push(...collectHtmlFiles(fullPath));
    } else if (entry.endsWith(".html")) {
      results.push(fullPath);
    }
  }

  return results;
}

const base = normalizeBase(process.env.SITE_BASE);
const htmlFiles = collectHtmlFiles(distRoot);
const attrPattern = /\b(?:href|src|action)="([^"]+)"/g;
const violations = [];

for (const file of htmlFiles) {
  const html = readFileSync(file, "utf8");
  for (const match of html.matchAll(attrPattern)) {
    const url = match[1];
    if (
      url.startsWith("#") ||
      url.startsWith("//") ||
      url.startsWith("http://") ||
      url.startsWith("https://") ||
      url.startsWith("mailto:") ||
      url.startsWith("tel:") ||
      url.startsWith("data:") ||
      url.startsWith("javascript:")
    ) {
      continue;
    }

    if (!isAllowedRootRelative(url, base)) {
      violations.push(`${path.relative(distRoot, file)}: ${url}`);
    }
  }
}

if (violations.length > 0) {
  console.error("Found built links that escape the configured site base:");
  for (const violation of violations) {
    console.error(`  - ${violation}`);
  }
  process.exit(1);
}

console.log(
  `Verified ${htmlFiles.length} built HTML files against site base ${JSON.stringify(base)}`
);
