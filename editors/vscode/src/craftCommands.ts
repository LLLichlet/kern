import * as path from "node:path";

export type CraftBuildPackageArgs = {
    manifestPath?: string;
};

export type CraftTestTargetArgs = {
    manifestPath?: string;
    targetName?: string;
};

export function parseCraftBuildPackageArgs(raw: unknown): CraftBuildPackageArgs {
    if (!raw || typeof raw !== "object") {
        return {};
    }
    const value = raw as Record<string, unknown>;
    return {
        manifestPath:
            typeof value.manifestPath === "string" ? value.manifestPath : undefined,
    };
}

export function parseCraftTestTargetArgs(raw: unknown): CraftTestTargetArgs {
    if (!raw || typeof raw !== "object") {
        return {};
    }
    const value = raw as Record<string, unknown>;
    return {
        manifestPath:
            typeof value.manifestPath === "string" ? value.manifestPath : undefined,
        targetName: typeof value.targetName === "string" ? value.targetName : undefined,
    };
}

export function manifestWorkingDirectory(
    manifestPath: string,
    nodePlatform: NodeJS.Platform = process.platform,
): string {
    if (!hasPathSeparator(manifestPath)) {
        return ".";
    }

    const dirname = pathApiFor(nodePlatform).dirname(manifestPath);
    return dirname.length > 0 ? dirname : ".";
}

export function taskEnvironment(env: NodeJS.ProcessEnv): Record<string, string> {
    const clean: Record<string, string> = {};
    for (const [key, value] of Object.entries(env)) {
        if (typeof value === "string") {
            clean[key] = value;
        }
    }
    return clean;
}

export function isPathWithin(
    candidatePath: string,
    root: string,
    nodePlatform: NodeJS.Platform = process.platform,
): boolean {
    const pathApi = pathApiFor(nodePlatform);
    const normalizedPath = normalizePathForComparison(
        pathApi.normalize(candidatePath),
        nodePlatform,
    );
    const normalizedRoot = normalizePathForComparison(
        pathApi.normalize(root),
        nodePlatform,
    ).replace(/[\\/]+$/, "");
    const rootWithSeparator = `${normalizedRoot}${pathApi.sep}`;

    return (
        normalizedPath === normalizedRoot ||
        normalizedPath.startsWith(rootWithSeparator) ||
        (normalizedRoot.length === 0 && normalizedPath.startsWith(pathApi.sep))
    );
}

function normalizePathForComparison(
    value: string,
    nodePlatform: NodeJS.Platform,
): string {
    return nodePlatform === "win32" ? value.toLowerCase() : value;
}

function hasPathSeparator(value: string): boolean {
    return value.includes("/") || value.includes("\\");
}

function pathApiFor(nodePlatform: NodeJS.Platform): typeof path.posix {
    return nodePlatform === "win32" ? path.win32 : path.posix;
}
