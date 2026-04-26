import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import {
    resolveConfiguredServerPath,
    resolveServerCommand,
    type ResolveServerCommandOptions,
} from "../serverResolution";

function existsIn(paths: string[]): (candidate: string) => boolean {
    const known = new Set(paths);
    return (candidate) => known.has(candidate);
}

function options(
    overrides: Partial<ResolveServerCommandOptions> = {},
): ResolveServerCommandOptions {
    return {
        configuredPath: "",
        configuredToolchainPath: "",
        configuredArgs: [],
        workspaceRoots: [{ fsPath: "/workspace", name: "demo" }],
        extensionPath: "/extension",
        env: {},
        homeDir: "/home/alice",
        nodePlatform: "linux",
        nodeArch: "x64",
        ...overrides,
    };
}

test("configured relative path resolves against the first workspace root", () => {
    const serverPath = path.posix.join("/workspace", "bin", "kern-lsp");
    const resolution = resolveServerCommand(
        options({
            configuredPath: "bin/kern-lsp",
            configuredArgs: ["--trace"],
        }),
        existsIn([serverPath]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: serverPath,
        args: ["--trace"],
        source: "configured path",
    });
});

test("configured filesystem path fails without falling back", () => {
    const resolution = resolveServerCommand(
        options({ configuredPath: "/missing/kern-lsp" }),
        existsIn([]),
    );

    assert.deepEqual(resolution, {
        kind: "error",
        message: "Configured kern-lsp path does not exist: /missing/kern-lsp",
    });
});

test("configured toolchain root resolves to bin/kern-lsp", () => {
    const serverPath = "/opt/kern/bin/kern-lsp";
    const resolution = resolveServerCommand(
        options({ configuredToolchainPath: "/opt/kern" }),
        existsIn([serverPath]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: serverPath,
        args: [],
        source: "configured toolchain",
    });
});

test("configured toolchain root fails when kern-lsp is absent", () => {
    const resolution = resolveServerCommand(
        options({ configuredToolchainPath: "/opt/kern" }),
        existsIn([]),
    );

    assert.deepEqual(resolution, {
        kind: "error",
        message: "Configured Kern toolchain does not contain kern-lsp: /opt/kern/bin/kern-lsp",
    });
});

test("PATH server wins before installed and workspace candidates", () => {
    const pathServer = "/repo/kern/target/release/kern-lsp";
    const installed = "/home/alice/.kern/bin/kern-lsp";
    const workspaceRelease = "/workspace/target/release/kern-lsp";
    const resolution = resolveServerCommand(
        options({
            configuredArgs: ["--stdio"],
            env: { PATH: "/repo/kern/target/release:/usr/bin" },
        }),
        existsIn([pathServer, installed, workspaceRelease]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: pathServer,
        args: ["--stdio"],
        source: "PATH",
    });
});

test("KERN_HOME install is used when PATH does not contain kern-lsp", () => {
    const installed = "/sdk/kern/bin/kern-lsp";
    const resolution = resolveServerCommand(
        options({
            env: { PATH: "/usr/bin", KERN_HOME: "/sdk/kern" },
        }),
        existsIn([installed]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: installed,
        args: [],
        source: "installed toolchain",
    });
});

test("default home install is used without KERN_HOME", () => {
    const installed = "/home/alice/.kern/bin/kern-lsp";
    const resolution = resolveServerCommand(
        options({ env: { PATH: "/usr/bin" } }),
        existsIn([installed]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: installed,
        args: [],
        source: "installed toolchain",
    });
});

test("workspace release build wins before debug when no toolchain is found", () => {
    const workspaceDebug = "/workspace/target/debug/kern-lsp";
    const workspaceRelease = "/workspace/target/release/kern-lsp";
    const resolution = resolveServerCommand(
        options({ env: { PATH: "/usr/bin" } }),
        existsIn([workspaceDebug, workspaceRelease]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: workspaceRelease,
        args: [],
        source: "workspace demo",
    });
});

test("falls back to PATH command when no concrete server is found", () => {
    const resolution = resolveServerCommand(
        options({
            configuredArgs: ["--library-bundle", "std"],
            env: { PATH: "/usr/bin" },
        }),
        existsIn([]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: "kern-lsp",
        args: ["--library-bundle", "std"],
        source: "PATH",
    });
});

test("windows targets use .exe server names and path separators", () => {
    const pathServer = "C:\\kern\\bin\\kern-lsp.exe";
    const resolution = resolveServerCommand(
        options({
            workspaceRoots: [],
            extensionPath: "C:\\extension",
            env: { Path: "C:\\kern\\bin;C:\\Windows" },
            homeDir: "C:\\Users\\alice",
            nodePlatform: "win32",
            nodeArch: "x64",
        }),
        existsIn([pathServer]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: pathServer,
        args: [],
        source: "PATH",
    });
});

test("relative configured paths stay relative without a workspace", () => {
    assert.equal(resolveConfiguredServerPath("bin/kern-lsp"), "bin/kern-lsp");
});
