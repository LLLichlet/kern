import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import {
    detectServerPlatform,
    resolveConfiguredServerPath,
    resolveServerCommand,
} from "../serverResolution";

function existsIn(paths: string[]): (candidate: string) => boolean {
    const known = new Set(paths);
    return (candidate) => known.has(candidate);
}

test("configured relative path resolves against the first workspace root", () => {
    const serverPath = path.posix.join("/workspace", "bin", "kern-lsp");
    const resolution = resolveServerCommand(
        {
            configuredPath: "bin/kern-lsp",
            configuredArgs: ["--trace"],
            workspaceRoots: [{ fsPath: "/workspace", name: "demo" }],
            extensionPath: "/extension",
            nodePlatform: "linux",
            nodeArch: "x64",
        },
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
        {
            configuredPath: "/missing/kern-lsp",
            configuredArgs: [],
            workspaceRoots: [{ fsPath: "/workspace", name: "demo" }],
            extensionPath: "/extension",
            nodePlatform: "linux",
            nodeArch: "x64",
        },
        existsIn([]),
    );

    assert.deepEqual(resolution, {
        kind: "error",
        message: "Configured kern-lsp path does not exist: /missing/kern-lsp",
    });
});

test("bundled server wins before workspace binaries", () => {
    const bundled = "/extension/server/linux-x64/kern-lsp";
    const workspaceDebug = "/workspace/target/debug/kern-lsp";
    const resolution = resolveServerCommand(
        {
            configuredPath: "",
            configuredArgs: ["--stdio"],
            workspaceRoots: [{ fsPath: "/workspace", name: "demo" }],
            extensionPath: "/extension",
            nodePlatform: "linux",
            nodeArch: "x64",
        },
        existsIn([bundled, workspaceDebug]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: bundled,
        args: ["--stdio"],
        source: "bundled linux-x64",
    });
});

test("workspace debug build wins before release and PATH", () => {
    const workspaceDebug = "/workspace/target/debug/kern-lsp";
    const workspaceRelease = "/workspace/target/release/kern-lsp";
    const resolution = resolveServerCommand(
        {
            configuredPath: "",
            configuredArgs: [],
            workspaceRoots: [{ fsPath: "/workspace", name: "demo" }],
            extensionPath: "/extension",
            nodePlatform: "linux",
            nodeArch: "x64",
        },
        existsIn([workspaceDebug, workspaceRelease]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: workspaceDebug,
        args: [],
        source: "workspace demo",
    });
});

test("falls back to PATH when no concrete server is found", () => {
    const resolution = resolveServerCommand(
        {
            configuredPath: "",
            configuredArgs: ["--library-bundle", "std"],
            workspaceRoots: [{ fsPath: "/workspace", name: "demo" }],
            extensionPath: "/extension",
            nodePlatform: "linux",
            nodeArch: "x64",
        },
        existsIn([]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: "kern-lsp",
        args: ["--library-bundle", "std"],
        source: "PATH",
    });
});

test("windows targets use .exe server names", () => {
    const bundled = "C:\\extension\\server\\win32-x64\\kern-lsp.exe";
    const resolution = resolveServerCommand(
        {
            configuredPath: "",
            configuredArgs: [],
            workspaceRoots: [],
            extensionPath: "C:\\extension",
            nodePlatform: "win32",
            nodeArch: "x64",
        },
        existsIn([bundled]),
    );

    assert.deepEqual(resolution, {
        kind: "resolved",
        command: bundled,
        args: [],
        source: "bundled win32-x64",
    });
});

test("platform detection only allows packaged targets", () => {
    assert.equal(detectServerPlatform("linux", "x64"), "linux-x64");
    assert.equal(detectServerPlatform("linux", "ppc64"), undefined);
});

test("relative configured paths stay relative without a workspace", () => {
    assert.equal(resolveConfiguredServerPath("bin/kern-lsp"), "bin/kern-lsp");
});
