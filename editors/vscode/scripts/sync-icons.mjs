import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const extensionRoot = path.resolve(scriptDir, "..");
const repoRoot = path.resolve(extensionRoot, "..", "..");

const sourceSvg = path.join(repoRoot, "logo.svg");
const sourcePng = path.join(repoRoot, "logo.png");
const targetSvg = path.join(extensionRoot, "icons", "kern.svg");
const targetPng = path.join(extensionRoot, "icons", "kern.png");

syncSvg();
syncPng();

function syncSvg() {
    const source = fs.readFileSync(sourceSvg, "utf8");
    const existing = fs.existsSync(targetSvg)
        ? fs.readFileSync(targetSvg, "utf8")
        : null;

    if (existing === source) {
        console.log("[kern-vscode] icons/kern.svg already matches ../../logo.svg");
        return;
    }

    fs.writeFileSync(targetSvg, source);
    console.log("[kern-vscode] synced icons/kern.svg from ../../logo.svg");
}

function syncPng() {
    if (!fs.existsSync(sourcePng)) {
        console.warn(
            "[kern-vscode] warning: ../../logo.png is missing; icons/kern.png was left unchanged.",
        );
        return;
    }

    const source = fs.readFileSync(sourcePng);
    const existing = fs.existsSync(targetPng) ? fs.readFileSync(targetPng) : null;
    if (existing && Buffer.compare(existing, source) === 0) {
        console.log("[kern-vscode] icons/kern.png already matches ../../logo.png");
        return;
    }

    fs.writeFileSync(targetPng, source);
    console.log("[kern-vscode] synced icons/kern.png from ../../logo.png");
}
