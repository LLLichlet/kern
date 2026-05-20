import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { ZipFile } = require("yazl");
const {
    createDefaultProcessors,
    processFiles,
    readManifest,
} = require("@vscode/vsce/out/package.js");
const { filePathToVsixPath } = require("@vscode/vsce/out/util.js");

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const extensionRoot = path.resolve(scriptDir, "..");
const packageJson = JSON.parse(
    fs.readFileSync(path.join(extensionRoot, "package.json"), "utf8"),
);
const args = parseArgs(process.argv.slice(2));

const out =
    args.out ??
    path.join(extensionRoot, `${packageJson.name}-${packageJson.version}.vsix`);

cleanServerDirectory();
runNpm(["run", "compile"], extensionRoot);

const manifest = await readManifest(extensionRoot);
const fileNames = listPackFiles();
const rawFiles = fileNames.map((file) => ({
    path: filePathToVsixPath(file),
    localPath: path.join(extensionRoot, file),
}));
const files = await processFiles(createDefaultProcessors(manifest), rawFiles);
await writeVsix(files, out);
console.log(`[kern-language] packaged ${out}`);

function cleanServerDirectory() {
    fs.rmSync(path.join(extensionRoot, "server"), { recursive: true, force: true });
}

function writeVsix(files, packagePath) {
    fs.rmSync(packagePath, { force: true });

    return new Promise((resolve, reject) => {
        const zip = new ZipFile();
        const output = fs.createWriteStream(packagePath);

        zip.outputStream.pipe(output);
        zip.outputStream.once("error", reject);
        output.once("error", reject);
        output.once("finish", resolve);

        for (const file of files) {
            if ("contents" in file) {
                const contents =
                    typeof file.contents === "string"
                        ? Buffer.from(file.contents, "utf8")
                        : file.contents;
                zip.addBuffer(contents, file.path, { mode: file.mode });
            } else {
                zip.addFile(file.localPath, file.path, { mode: file.mode });
            }
        }

        zip.end();
    });
}

function listPackFiles() {
    const files = new Set(listRootFiles(extensionRoot));
    for (const file of listProductionDependencyFiles()) {
        files.add(file);
    }
    validateRuntimeFiles(files);
    return [...files].sort();
}

function listRootFiles(root) {
    const files = [];
    collectRootFiles(root, "", files);
    return files;
}

function collectRootFiles(root, relativeDir, files) {
    const directory = relativeDir ? path.join(root, relativeDir) : root;
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
        const relativePath = relativeDir
            ? `${relativeDir}/${entry.name}`
            : entry.name;

        if (shouldIgnoreRootPath(relativePath, entry.isDirectory())) {
            continue;
        }

        const absolutePath = path.join(root, relativePath);
        if (entry.isDirectory()) {
            collectRootFiles(root, relativePath, files);
            continue;
        }

        if (entry.isFile()) {
            files.push(relativePath);
        }
    }
}

function shouldIgnoreRootPath(relativePath, isDirectory) {
    if (
        relativePath === ".gitignore" ||
        relativePath === ".npmignore" ||
        relativePath === ".vscodeignore" ||
        relativePath === "package-lock.json" ||
        relativePath === "tsconfig.json"
    ) {
        return true;
    }

    if (
        relativePath === ".vscode" ||
        relativePath === "src" ||
        relativePath === "scripts" ||
        relativePath === "server" ||
        relativePath === "testdata"
    ) {
        return true;
    }

    if (
        relativePath.startsWith(".vscode/") ||
        relativePath.startsWith("src/") ||
        relativePath.startsWith("scripts/") ||
        relativePath.startsWith("server/") ||
        relativePath.startsWith("testdata/")
    ) {
        return true;
    }

    if (relativePath.endsWith(".tsbuildinfo") || relativePath.endsWith(".vsix")) {
        return true;
    }

    if (relativePath.startsWith("out/") && relativePath.endsWith(".map")) {
        return true;
    }

    if (
        relativePath.startsWith("out/") &&
        relativePath.endsWith(".js") &&
        relativePath !== "out/extension.js"
    ) {
        return true;
    }

    if (relativePath === "out/test" || relativePath.startsWith("out/test/")) {
        return true;
    }

    return isDirectory && relativePath === "node_modules";
}

function listProductionDependencyFiles() {
    const lockPath = path.join(extensionRoot, "package-lock.json");
    if (!fs.existsSync(lockPath)) {
        fail("package-lock.json is required to package runtime dependencies");
    }

    const lockfile = JSON.parse(fs.readFileSync(lockPath, "utf8"));
    const packages = lockfile.packages;
    if (!packages || typeof packages !== "object") {
        fail("package-lock.json does not contain a packages map");
    }

    const dependencies = Object.entries(packages)
        .filter(([relativePath, metadata]) => {
            return (
                relativePath.startsWith("node_modules/") &&
                metadata &&
                typeof metadata === "object" &&
                metadata.dev !== true
            );
        })
        .map(([relativePath]) => path.join(extensionRoot, relativePath))
        .filter((entry) => fs.existsSync(entry));

    const files = [];
    for (const dependency of dependencies) {
        collectDependencyFiles(dependency, files);
    }
    return files;
}

function collectDependencyFiles(root, files) {
    for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
        const absolute = path.join(root, entry.name);
        if (entry.isDirectory()) {
            if (entry.name === "node_modules") {
                continue;
            }
            collectDependencyFiles(absolute, files);
            continue;
        }

        if (!entry.isFile()) {
            continue;
        }

        files.push(path.relative(extensionRoot, absolute).replaceAll(path.sep, "/"));
    }
}

function run(command, commandArgs, cwd, extraEnv = {}) {
    const result = spawnSync(command, commandArgs, {
        cwd,
        stdio: "inherit",
        env: {
            ...process.env,
            ...extraEnv,
        },
    });

    if (result.status !== 0) {
        process.exit(result.status ?? 1);
    }
}

function runNpm(commandArgs, cwd, extraEnv = {}) {
    const npmExecPath = process.env.npm_execpath;
    if (npmExecPath) {
        run(process.execPath, [npmExecPath, ...commandArgs], cwd, extraEnv);
        return;
    }

    run("npm", commandArgs, cwd, extraEnv);
}

function validateRuntimeFiles(files) {
    if (Object.keys(packageJson.dependencies ?? {}).length === 0) {
        return;
    }

    const hasRuntimeDependencies = [...files].some((file) =>
        file.startsWith("node_modules/"),
    );

    if (!hasRuntimeDependencies) {
        fail("packaged extension is missing runtime node_modules contents");
    }
}

function parseArgs(argv) {
    const parsed = {};
    for (let index = 0; index < argv.length; index += 1) {
        const arg = argv[index];
        if (!arg.startsWith("--")) {
            fail(`unsupported argument \`${arg}\``);
        }

        const key = arg.slice(2);
        if (key !== "out") {
            fail(`unsupported argument \`${arg}\``);
        }
        const value = argv[index + 1];
        if (!value || value.startsWith("--")) {
            fail(`expected a value after \`${arg}\``);
        }

        parsed[key] = value;
        index += 1;
    }
    return parsed;
}

function fail(message) {
    console.error(`[kern-language] ${message}`);
    process.exit(1);
}
