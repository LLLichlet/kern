import * as path from "node:path";

export interface ResolvedServerCommand {
    kind: "resolved";
    command: string;
    args: string[];
    source: string;
}

export interface UnresolvedServerCommand {
    kind: "error";
    message: string;
}

export type ServerResolutionResult = ResolvedServerCommand | UnresolvedServerCommand;

export interface ResolveServerCommandOptions {
    configuredPath: string;
    configuredArgs: string[];
    workspaceRoots: WorkspaceRoot[];
    extensionPath: string;
    nodePlatform?: NodeJS.Platform;
    nodeArch?: string;
}

export interface WorkspaceRoot {
    fsPath: string;
    name: string;
}

export function resolveServerCommand(
    options: ResolveServerCommandOptions,
    existsSync: (candidate: string) => boolean,
): ServerResolutionResult {
    const nodePlatform = options.nodePlatform ?? process.platform;
    const nodeArch = options.nodeArch ?? process.arch;
    const configuredPath = options.configuredPath.trim();

    if (configuredPath.length > 0) {
        const resolvedPath = resolveConfiguredServerPath(
            configuredPath,
            options.workspaceRoots[0]?.fsPath,
            nodePlatform,
        );
        if (looksLikeFilesystemPath(resolvedPath, nodePlatform) && !existsSync(resolvedPath)) {
            return {
                kind: "error",
                message: `Configured kern-lsp path does not exist: ${resolvedPath}`,
            };
        }

        return {
            kind: "resolved",
            command: resolvedPath,
            args: options.configuredArgs,
            source: "configured path",
        };
    }

    const bundledPath = bundledServerCandidate(
        options.extensionPath,
        nodePlatform,
        nodeArch,
    );
    if (bundledPath && existsSync(bundledPath)) {
        return {
            kind: "resolved",
            command: bundledPath,
            args: options.configuredArgs,
            source: `bundled ${detectServerPlatform(nodePlatform, nodeArch)}`,
        };
    }

    for (const folder of options.workspaceRoots) {
        for (const candidate of localServerCandidates(folder.fsPath, nodePlatform)) {
            if (existsSync(candidate)) {
                return {
                    kind: "resolved",
                    command: candidate,
                    args: options.configuredArgs,
                    source: `workspace ${folder.name}`,
                };
            }
        }
    }

    return {
        kind: "resolved",
        command: executableName("kern-lsp", nodePlatform),
        args: options.configuredArgs,
        source: "PATH",
    };
}

export function executableName(
    base: string,
    nodePlatform: NodeJS.Platform = process.platform,
): string {
    return nodePlatform === "win32" ? `${base}.exe` : base;
}

export function detectServerPlatform(
    nodePlatform: NodeJS.Platform = process.platform,
    nodeArch: string = process.arch,
): string | undefined {
    const candidate = `${nodePlatform}-${nodeArch}`;
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

export function bundledServerCandidate(
    extensionPath: string,
    nodePlatform: NodeJS.Platform = process.platform,
    nodeArch: string = process.arch,
): string | undefined {
    const bundleId = detectServerPlatform(nodePlatform, nodeArch);
    if (!bundleId) {
        return undefined;
    }

    return pathApiFor(nodePlatform).join(
        extensionPath,
        "server",
        bundleId,
        executableName("kern-lsp", nodePlatform),
    );
}

export function localServerCandidates(
    root: string,
    nodePlatform: NodeJS.Platform = process.platform,
): string[] {
    const name = executableName("kern-lsp", nodePlatform);
    const pathApi = pathApiFor(nodePlatform);
    return [
        pathApi.join(root, "target", "debug", name),
        pathApi.join(root, "target", "release", name),
    ];
}

export function resolveConfiguredServerPath(
    configuredPath: string,
    workspaceRoot?: string,
    nodePlatform: NodeJS.Platform = process.platform,
): string {
    const pathApi = pathApiFor(nodePlatform);
    if (pathApi.isAbsolute(configuredPath) || !hasPathSeparator(configuredPath)) {
        return configuredPath;
    }

    if (!workspaceRoot) {
        return configuredPath;
    }

    return pathApi.resolve(workspaceRoot, configuredPath);
}

function looksLikeFilesystemPath(
    value: string,
    nodePlatform: NodeJS.Platform = process.platform,
): boolean {
    return pathApiFor(nodePlatform).isAbsolute(value) || hasPathSeparator(value);
}

function hasPathSeparator(value: string): boolean {
    return value.includes("/") || value.includes("\\");
}

function pathApiFor(nodePlatform: NodeJS.Platform): typeof path.posix {
    return nodePlatform === "win32" ? path.win32 : path.posix;
}
