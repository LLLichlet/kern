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
    configuredToolchainPath: string;
    configuredArgs: string[];
    workspaceRoots: WorkspaceRoot[];
    extensionPath: string;
    env?: NodeJS.ProcessEnv;
    homeDir?: string;
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
    const env = options.env ?? process.env;
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

    const configuredToolchainPath = options.configuredToolchainPath.trim();
    if (configuredToolchainPath.length > 0) {
        const root = resolveConfiguredServerPath(
            configuredToolchainPath,
            options.workspaceRoots[0]?.fsPath,
            nodePlatform,
        );
        const serverPath = toolchainServerCandidate(root, nodePlatform);
        if (!existsSync(serverPath)) {
            return {
                kind: "error",
                message: `Configured Kern toolchain does not contain kern-lsp: ${serverPath}`,
            };
        }

        return {
            kind: "resolved",
            command: serverPath,
            args: options.configuredArgs,
            source: "configured toolchain",
        };
    }

    const pathServer = findExecutableOnPath(
        executableName("kern-lsp", nodePlatform),
        env,
        nodePlatform,
        existsSync,
    );
    if (pathServer) {
        return {
            kind: "resolved",
            command: pathServer,
            args: options.configuredArgs,
            source: "PATH",
        };
    }

    for (const candidate of installedToolchainCandidates(options, env, nodePlatform)) {
        if (existsSync(candidate)) {
            return {
                kind: "resolved",
                command: candidate,
                args: options.configuredArgs,
                source: "installed toolchain",
            };
        }
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

export function localServerCandidates(
    root: string,
    nodePlatform: NodeJS.Platform = process.platform,
): string[] {
    const name = executableName("kern-lsp", nodePlatform);
    const pathApi = pathApiFor(nodePlatform);
    return [
        pathApi.join(root, "target", "release", name),
        pathApi.join(root, "target", "debug", name),
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

function installedToolchainCandidates(
    options: ResolveServerCommandOptions,
    env: NodeJS.ProcessEnv,
    nodePlatform: NodeJS.Platform,
): string[] {
    const roots: string[] = [];
    const kernHome = env.KERN_HOME?.trim();
    if (kernHome) {
        roots.push(kernHome);
    }
    const homeDir = options.homeDir;
    if (homeDir) {
        roots.push(pathApiFor(nodePlatform).join(homeDir, ".kern"));
    }

    const seen = new Set<string>();
    const candidates: string[] = [];
    for (const root of roots) {
        const candidate = toolchainServerCandidate(root, nodePlatform);
        if (!seen.has(candidate)) {
            seen.add(candidate);
            candidates.push(candidate);
        }
    }
    return candidates;
}

function toolchainServerCandidate(
    root: string,
    nodePlatform: NodeJS.Platform,
): string {
    return pathApiFor(nodePlatform).join(
        root,
        "bin",
        executableName("kern-lsp", nodePlatform),
    );
}

function findExecutableOnPath(
    executable: string,
    env: NodeJS.ProcessEnv,
    nodePlatform: NodeJS.Platform,
    existsSync: (candidate: string) => boolean,
): string | undefined {
    const pathValue = pathEnvValue(env);
    if (!pathValue) {
        return undefined;
    }

    const pathApi = pathApiFor(nodePlatform);
    for (const entry of pathValue.split(pathDelimiterFor(nodePlatform))) {
        if (!entry) {
            continue;
        }
        const candidate = pathApi.join(entry, executable);
        if (existsSync(candidate)) {
            return candidate;
        }
    }

    return undefined;
}

function pathEnvValue(env: NodeJS.ProcessEnv): string | undefined {
    return env.PATH ?? env.Path ?? env.path;
}

function pathDelimiterFor(nodePlatform: NodeJS.Platform): string {
    return nodePlatform === "win32" ? ";" : ":";
}

function hasPathSeparator(value: string): boolean {
    return value.includes("/") || value.includes("\\");
}

function pathApiFor(nodePlatform: NodeJS.Platform): typeof path.posix {
    return nodePlatform === "win32" ? path.win32 : path.posix;
}
