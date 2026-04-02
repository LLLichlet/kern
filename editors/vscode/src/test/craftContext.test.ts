import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import {
    craftRefreshArgs,
    discoverCraftWorkspaceFolders,
    resolveCraftCommand,
} from "../craftContext";

function existsIn(paths: string[]): (candidate: string) => boolean {
    const known = new Set(paths);
    return (candidate) => known.has(candidate);
}

test("craft refresh args include normalized feature selection", () => {
    assert.deepEqual(craftRefreshArgs([" experimental ", "simd"], false), [
        "check",
        "--features",
        "experimental,simd",
    ]);
});

test("craft refresh args include no-default-features when configured", () => {
    assert.deepEqual(craftRefreshArgs([], true), ["check", "--no-default-features"]);
});

test("configured relative craft path resolves against workspace root", () => {
    assert.equal(
        resolveCraftCommand("bin/craft", "/workspace"),
        path.posix.resolve("/workspace", "bin/craft"),
    );
});

test("empty craft path falls back to PATH executable name", () => {
    assert.equal(resolveCraftCommand(""), "craft");
});

test("discover craft workspace folders keeps only folders with Craft.toml", () => {
    const folders = [
        { fsPath: "/workspace/app", name: "app" },
        { fsPath: "/workspace/docs", name: "docs" },
    ];
    const discovered = discoverCraftWorkspaceFolders(
        folders,
        existsIn([path.posix.join("/workspace/app", "Craft.toml")]),
        "linux",
    );

    assert.deepEqual(discovered, [{ fsPath: "/workspace/app", name: "app" }]);
});
