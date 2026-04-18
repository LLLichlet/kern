import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

function resolveBrandLogoPath() {
  const candidates = [
    path.resolve(process.cwd(), "logo.svg"),
    path.resolve(process.cwd(), "..", "logo.svg")
  ];

  const match = candidates.find((candidate) => existsSync(candidate));

  if (!match) {
    throw new Error(`Unable to locate Kern logo.svg. Checked: ${candidates.join(", ")}`);
  }

  return match;
}

export function loadBrandSvg() {
  return readFileSync(resolveBrandLogoPath(), "utf8");
}
