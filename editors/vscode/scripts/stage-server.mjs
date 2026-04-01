import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const extensionRoot = path.resolve(scriptDir, "..");
const repoRoot = path.resolve(extensionRoot, "..", "..");
const bundleId = process.env.KERN_VSCODE_SERVER_PLATFORM ?? platformId();
const binaryName = executableName("kern-lsp", bundleId);

if (!bundleId) {
    fail(
        `unsupported platform \`${process.platform}-${process.arch}\`; set KERN_VSCODE_SERVER_PLATFORM explicitly`,
    );
}

const source = resolveSourceBinary(binaryName);
if (!source) {
    fail(
        "could not find a kern-lsp binary to stage; build `cargo build -p kern-lsp --release` or set KERN_VSCODE_SERVER_SOURCE",
    );
}

const destination = path.join(extensionRoot, "server", bundleId, binaryName);
fs.mkdirSync(path.dirname(destination), { recursive: true });
fs.copyFileSync(source, destination);

if (!binaryName.endsWith(".exe")) {
    fs.chmodSync(destination, 0o755);
}

console.log(`[kern-vscode] staged ${source} -> ${destination}`);

function resolveSourceBinary(binaryName) {
    const explicit = process.env.KERN_VSCODE_SERVER_SOURCE;
    const candidates = explicit
        ? [explicit]
        : [
              path.join(repoRoot, "target", "release", binaryName),
              path.join(repoRoot, "target", "debug", binaryName),
          ];

    for (const candidate of candidates) {
        if (fs.existsSync(candidate)) {
            return candidate;
        }
    }

    return undefined;
}

function executableName(base, bundleId) {
    return bundleId.startsWith("win32-") ? `${base}.exe` : base;
}

function platformId() {
    const candidate = `${process.platform}-${process.arch}`;
    switch (candidate) {
        case "darwin-arm64":
        case "darwin-x64":
        case "linux-arm64":
        case "linux-x64":
        case "win32-x64":
            return candidate;
        default:
            return undefined;
    }
}

function fail(message) {
    console.error(`[kern-vscode] ${message}`);
    process.exit(1);
}
