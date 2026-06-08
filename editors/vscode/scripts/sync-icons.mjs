import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const extensionRoot = path.resolve(scriptDir, "..");
const repoRoot = path.resolve(extensionRoot, "..", "..");

const sourceLightSvg = path.join(
    repoRoot,
    "assets",
    "file-icons",
    "kern-file-icon-light-theme.svg",
);
const sourceDarkSvg = path.join(
    repoRoot,
    "assets",
    "file-icons",
    "kern-file-icon-dark-theme.svg",
);
const sourcePng = path.join(repoRoot, "assets", "brand", "kern-mark.png");
const targetLightSvg = path.join(extensionRoot, "icons", "kern-light.svg");
const targetDarkSvg = path.join(extensionRoot, "icons", "kern-dark.svg");
const targetPng = path.join(extensionRoot, "icons", "kern.png");

syncText(sourceLightSvg, targetLightSvg, "../../assets/file-icons/kern-file-icon-light-theme.svg");
syncText(sourceDarkSvg, targetDarkSvg, "../../assets/file-icons/kern-file-icon-dark-theme.svg");
syncPng();

function syncText(sourcePath, targetPath, label) {
    const source = fs.readFileSync(sourcePath, "utf8");
    const existing = fs.existsSync(targetPath)
        ? fs.readFileSync(targetPath, "utf8")
        : null;

    if (existing === source) {
        console.log(`[kern-language] ${relativeToExtension(targetPath)} already matches ${label}`);
        return;
    }

    fs.writeFileSync(targetPath, source);
    console.log(`[kern-language] synced ${relativeToExtension(targetPath)} from ${label}`);
}

function syncPng() {
    if (!fs.existsSync(sourcePng)) {
        console.warn(
            "[kern-language] warning: ../../assets/brand/kern-mark.png is missing; icons/kern.png was left unchanged.",
        );
        return;
    }

    const source = fs.readFileSync(sourcePng);
    const existing = fs.existsSync(targetPng) ? fs.readFileSync(targetPng) : null;
    if (existing && Buffer.compare(existing, source) === 0) {
        console.log("[kern-language] icons/kern.png already matches ../../assets/brand/kern-mark.png");
        return;
    }

    fs.writeFileSync(targetPng, source);
    console.log("[kern-language] synced icons/kern.png from ../../assets/brand/kern-mark.png");
}

function relativeToExtension(targetPath) {
    return path.relative(extensionRoot, targetPath).replaceAll(path.sep, "/");
}
