import * as path from "node:path";
import type { WorkspaceRoot } from "./serverResolution";

export function resolveCraftCommand(
    configuredPath: string,
    workspaceRoot?: string,
    nodePlatform: NodeJS.Platform = process.platform,
): string {
    const trimmed = configuredPath.trim();
    if (trimmed.length === 0) {
        return executableName("craft", nodePlatform);
    }

    const pathApi = pathApiFor(nodePlatform);
    if (pathApi.isAbsolute(trimmed) || !hasPathSeparator(trimmed) || !workspaceRoot) {
        return trimmed;
    }

    return pathApi.resolve(workspaceRoot, trimmed);
}

export function craftRefreshArgs(
    features: string[],
    noDefaultFeatures: boolean,
): string[] {
    const args = ["check"];
    const normalized = features
        .map((feature) => feature.trim())
        .filter((feature) => feature.length > 0);
    if (normalized.length > 0) {
        args.push("--features", normalized.join(","));
    }
    if (noDefaultFeatures) {
        args.push("--no-default-features");
    }
    return args;
}

export function discoverCraftWorkspaceFolders(
    workspaceRoots: WorkspaceRoot[],
    existsSync: (candidate: string) => boolean,
    nodePlatform: NodeJS.Platform = process.platform,
): WorkspaceRoot[] {
    const pathApi = pathApiFor(nodePlatform);
    return workspaceRoots.filter((root) => existsSync(pathApi.join(root.fsPath, "Craft.toml")));
}

function executableName(
    base: string,
    nodePlatform: NodeJS.Platform = process.platform,
): string {
    return nodePlatform === "win32" ? `${base}.exe` : base;
}

function hasPathSeparator(value: string): boolean {
    return value.includes("/") || value.includes("\\");
}

function pathApiFor(nodePlatform: NodeJS.Platform): typeof path.posix {
    return nodePlatform === "win32" ? path.win32 : path.posix;
}
