import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const extensionRoot = path.resolve(scriptDir, "..");

await build({
    entryPoints: [path.join(extensionRoot, "src", "extension.ts")],
    bundle: true,
    platform: "node",
    format: "cjs",
    target: "node20",
    outfile: path.join(extensionRoot, "out", "extension.js"),
    sourcemap: true,
    external: ["vscode"],
    logLevel: "info",
});
