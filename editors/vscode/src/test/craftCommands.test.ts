import test from "node:test";
import assert from "node:assert/strict";
import {
    isPathWithin,
    manifestWorkingDirectory,
    parseCraftBuildPackageArgs,
    parseCraftRunTargetArgs,
    parseCraftTestTargetArgs,
    taskEnvironment,
} from "../craftCommands";

test("craft code lens command args are parsed defensively", () => {
    assert.deepEqual(parseCraftBuildPackageArgs(undefined), {});
    assert.deepEqual(parseCraftBuildPackageArgs({ manifestPath: "/app/Craft.toml" }), {
        manifestPath: "/app/Craft.toml",
    });
    assert.deepEqual(parseCraftBuildPackageArgs({ manifestPath: 7 }), {
        manifestPath: undefined,
    });

    assert.deepEqual(
        parseCraftRunTargetArgs({
            manifestPath: "/app/Craft.toml",
            targetKind: "example",
            targetName: "demo",
        }),
        {
            manifestPath: "/app/Craft.toml",
            targetKind: "example",
            targetName: "demo",
        },
    );
    assert.deepEqual(parseCraftRunTargetArgs({ targetKind: 7 }), {
        manifestPath: undefined,
        targetKind: undefined,
        targetName: undefined,
    });

    assert.deepEqual(
        parseCraftTestTargetArgs({
            manifestPath: "/app/Craft.toml",
            targetName: "smoke",
        }),
        {
            manifestPath: "/app/Craft.toml",
            targetName: "smoke",
        },
    );
    assert.deepEqual(parseCraftTestTargetArgs({ targetName: null }), {
        manifestPath: undefined,
        targetName: undefined,
    });
});

test("manifest working directory handles unix and windows roots", () => {
    assert.equal(manifestWorkingDirectory("/workspace/Craft.toml", "linux"), "/workspace");
    assert.equal(manifestWorkingDirectory("/Craft.toml", "linux"), "/");
    assert.equal(manifestWorkingDirectory("Craft.toml", "linux"), ".");
    assert.equal(
        manifestWorkingDirectory("C:\\workspace\\Craft.toml", "win32"),
        "C:\\workspace",
    );
    assert.equal(manifestWorkingDirectory("C:\\Craft.toml", "win32"), "C:\\");
});

test("task environment drops non-string process env values", () => {
    assert.deepEqual(taskEnvironment({ KERN_HOME: "/sdk", EMPTY: "", BAD: undefined }), {
        KERN_HOME: "/sdk",
        EMPTY: "",
    });
});

test("path containment matches platform path semantics", () => {
    assert.equal(isPathWithin("/workspace/app", "/workspace", "linux"), true);
    assert.equal(isPathWithin("/workspace/app", "/workspace/app", "linux"), true);
    assert.equal(isPathWithin("/workspace/app2", "/workspace/app", "linux"), false);
    assert.equal(isPathWithin("/Workspace/app", "/workspace", "linux"), false);

    assert.equal(
        isPathWithin("C:\\Workspace\\app", "c:\\workspace", "win32"),
        true,
    );
    assert.equal(
        isPathWithin("C:\\workspace-other", "C:\\workspace", "win32"),
        false,
    );
});
