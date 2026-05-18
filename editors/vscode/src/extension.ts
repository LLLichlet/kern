import * as fs from "node:fs";
import * as os from "node:os";
import { spawn } from "node:child_process";
import * as vscode from "vscode";
import {
    CloseAction,
    ErrorAction,
    LanguageClient,
    LanguageClientOptions,
    RevealOutputChannelOn,
    ServerOptions,
    State,
} from "vscode-languageclient/node";
import {
    resolveServerCommand,
    type ResolvedServerCommand,
} from "./serverResolution";
import {
    craftRefreshArgs,
    discoverCraftWorkspaceFolders,
    resolveCraftCommand,
} from "./craftContext";
import {
    type AutoSuggestMode,
    type AutoSuggestDocument,
    createAutoSuggestRequest,
    createAutoSuggestDocument,
    matchesAutoSuggestRequest,
    shouldAutoTriggerSuggest,
    shouldHideAutoTriggeredSuggest,
    updateAutoSuggestDocument,
} from "./clientBehavior";
import { DiagnosticBuffer } from "./diagnosticBuffer";

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel | undefined;
let statusItem: vscode.LanguageStatusItem | undefined;
let fileWatchers: vscode.FileSystemWatcher[] = [];
let diagnosticBuffer:
    | DiagnosticBuffer<vscode.Uri, vscode.Diagnostic>
    | undefined;
let pendingAutoSuggestRequest:
    | ReturnType<typeof createAutoSuggestRequest>
    | undefined;
let pendingAutoSuggestTimer: NodeJS.Timeout | undefined;
let autoSuggestDocuments = new Map<string, AutoSuggestDocument>();
let nextCraftTerminalId = 0;

const DIAGNOSTIC_DISPLAY_DELAY_MS = 180;
const AUTO_SUGGEST_DEBOUNCE_MS = 90;

type WorkspaceRoot = {
    fsPath: string;
    name: string;
};

type CraftBuildPackageArgs = {
    manifestPath?: string;
};

type CraftTestTargetArgs = {
    manifestPath?: string;
    targetName?: string;
};

const KERN_DOCUMENT_SELECTOR = [
    { scheme: "file", language: "kern" },
    { scheme: "untitled", language: "kern" },
];

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    outputChannel = vscode.window.createOutputChannel("Kern Language Server");
    statusItem = vscode.languages.createLanguageStatusItem(
        "kern.lsp.status",
        KERN_DOCUMENT_SELECTOR,
    );
    statusItem.name = "Kern Language Server";
    statusItem.command = {
        command: "kern.showLanguageServerOutput",
        title: "Show Kern Language Server Output",
    };
    context.subscriptions.push(outputChannel, statusItem);

    context.subscriptions.push(
        vscode.commands.registerCommand("kern.restartLanguageServer", async () => {
            await restartLanguageServer(context, true);
        }),
    );
    context.subscriptions.push(
        vscode.commands.registerCommand("kern.showLanguageServerOutput", () => {
            outputChannel?.show(true);
        }),
    );
    context.subscriptions.push(
        vscode.commands.registerCommand("kern.refreshCraftAnalysisContext", async () => {
            await refreshCraftAnalysisContext(context);
        }),
    );
    context.subscriptions.push(
        vscode.commands.registerCommand("kern.craft.buildPackage", async (args) => {
            await runCraftTargetCommand("build", args);
        }),
    );
    context.subscriptions.push(
        vscode.commands.registerCommand("kern.craft.testTarget", async (args) => {
            await runCraftTargetCommand("test", args);
        }),
    );

    context.subscriptions.push(
        vscode.workspace.onDidCloseTextDocument((document) => {
            autoSuggestDocuments.delete(document.uri.toString());
        }),
    );
    context.subscriptions.push(
        vscode.workspace.onDidChangeConfiguration((event) => {
            if (
                event.affectsConfiguration("kern.server") ||
                event.affectsConfiguration("kern.toolchain") ||
                event.affectsConfiguration("kern.project")
            ) {
                void restartLanguageServer(context, false);
            }
        }),
    );
    context.subscriptions.push(
        vscode.workspace.onDidChangeTextDocument((event) => {
            if (!client || client.state !== State.Running) {
                return;
            }

            const editor = vscode.window.activeTextEditor;
            if (!editor || editor.document !== event.document) {
                return;
            }
            if (event.document.languageId !== "kern") {
                return;
            }
            if (event.contentChanges.length !== 1) {
                autoSuggestDocuments.set(
                    event.document.uri.toString(),
                    createAutoSuggestDocument(event.document.getText()),
                );
                return;
            }

            cancelPendingAutoSuggest();

            const change = event.contentChanges[0];
            const documentUri = event.document.uri.toString();
            const autoSuggestDocument = autoSuggestDocuments.get(documentUri);
            const nextAutoSuggestDocument = autoSuggestDocument
                ? updateAutoSuggestDocument(
                      autoSuggestDocument,
                      {
                          line: change.range.start.line,
                          character: change.range.start.character,
                      },
                      {
                          line: change.range.end.line,
                          character: change.range.end.character,
                      },
                      change.text,
                  )
                : createAutoSuggestDocument(event.document.getText());
            autoSuggestDocuments.set(documentUri, nextAutoSuggestDocument);
            const insertedOffset =
                event.document.offsetAt(change.range.start) + change.text.length;
            const autoSuggestMode = vscode.workspace
                .getConfiguration("kern")
                .get<AutoSuggestMode>("editor.autoSuggest", "keywords");
            if (
                !shouldAutoTriggerSuggest(
                    autoSuggestMode,
                    change.text,
                    change.rangeLength ?? 0,
                    nextAutoSuggestDocument,
                    {
                        line: change.range.start.line,
                        character: change.range.start.character + change.text.length,
                    },
                )
            ) {
                return;
            }

            pendingAutoSuggestRequest = createAutoSuggestRequest(
                documentUri,
                event.document.version,
                insertedOffset,
                Date.now(),
            );
            const request = pendingAutoSuggestRequest;
            pendingAutoSuggestTimer = setTimeout(() => {
                pendingAutoSuggestTimer = undefined;
                const activeEditor = vscode.window.activeTextEditor;
                if (
                    !activeEditor ||
                    activeEditor.document.uri.toString() !== request.documentUri ||
                    activeEditor.document.version !== request.documentVersion
                ) {
                    return;
                }
                if (
                    !activeEditor.selection.isEmpty ||
                    activeEditor.document.offsetAt(activeEditor.selection.active) !== request.offset
                ) {
                    return;
                }
                if (pendingAutoSuggestRequest !== request) {
                    return;
                }
                void vscode.commands.executeCommand("editor.action.triggerSuggest");
            }, AUTO_SUGGEST_DEBOUNCE_MS);
        }),
    );

    await startLanguageServer(context);
}

export async function deactivate(): Promise<void> {
    await stopLanguageServer();
}

async function restartLanguageServer(
    context: vscode.ExtensionContext,
    explicit: boolean,
): Promise<void> {
    setStatus("restarting", "Restarting kern-lsp", vscode.LanguageStatusSeverity.Information);
    await stopLanguageServer();
    await startLanguageServer(context);

    if (explicit) {
        void vscode.window.showInformationMessage("Kern language server restarted.");
    }
}

async function startLanguageServer(context: vscode.ExtensionContext): Promise<void> {
    const kernConfig = vscode.workspace.getConfiguration("kern");
    const serverArgs = [
        ...projectAnalysisArgs(kernConfig),
        ...kernConfig.get<string[]>("server.args", []),
    ];
    const serverEnv = {
        ...process.env,
        ...configuredServerEnv(kernConfig),
    };
    const resolution = resolveServerCommand(
        {
            configuredPath: kernConfig.get<string>("server.path", ""),
            configuredToolchainPath: kernConfig.get<string>("toolchain.path", ""),
            configuredArgs: serverArgs,
            workspaceRoots: workspaceRoots(),
            extensionPath: context.extensionPath,
            env: serverEnv,
            homeDir: os.homedir(),
        },
        fs.existsSync,
    );
    if (resolution.kind === "error") {
        appendOutput(resolution.message);
        setStatus(
            "missing",
            "kern-lsp not found",
            vscode.LanguageStatusSeverity.Error,
        );
        void vscode.window.showErrorMessage(resolution.message);
        return;
    }

    const server = resolution;
    fileWatchers = createLanguageServerWatchers();
    diagnosticBuffer = new DiagnosticBuffer(DIAGNOSTIC_DISPLAY_DELAY_MS);
    cancelPendingAutoSuggest();
    appendOutput(
        `Starting kern-lsp (${server.source}): ${server.command} ${server.args.join(" ")}`.trim(),
    );
    setStatus("starting", "Starting kern-lsp", vscode.LanguageStatusSeverity.Information);

    const serverOptions: ServerOptions = {
        run: {
            command: server.command,
            args: server.args,
            options: { env: serverEnv },
        },
        debug: {
            command: server.command,
            args: server.args,
            options: { env: serverEnv },
        },
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: KERN_DOCUMENT_SELECTOR,
        outputChannel,
        revealOutputChannelOn: RevealOutputChannelOn.Never,
        initializationFailedHandler: (error) => {
            appendOutput(`kern-lsp initialization failed: ${formatError(error)}`);
            setStatus(
                "init-failed",
                "kern-lsp initialization failed",
                vscode.LanguageStatusSeverity.Error,
            );
            return false;
        },
        errorHandler: {
            error: (error, message, count) => {
                appendOutput(
                    `kern-lsp transport error${count ? ` #${count}` : ""}: ${formatError(error)}`,
                );
                if (message) {
                    appendOutput(`last protocol message: ${JSON.stringify(message)}`);
                }
                setStatus(
                    "io-error",
                    "kern-lsp transport error",
                    vscode.LanguageStatusSeverity.Warning,
                );
                return {
                    action: ErrorAction.Continue,
                    handled: true,
                };
            },
            closed: () => {
                appendOutput("kern-lsp connection closed. Restarting.");
                setStatus(
                    "reconnecting",
                    "Restarting kern-lsp",
                    vscode.LanguageStatusSeverity.Warning,
                );
                return {
                    action: CloseAction.Restart,
                    handled: true,
                };
            },
        },
        middleware: {
            provideCompletionItem: async (document, position, context, token, next) => {
                const result = await Promise.resolve(
                    next(document, position, context, token),
                );
                const offset = document.offsetAt(position);
                const autoSuggestMatched = matchesAutoSuggestRequest(
                    pendingAutoSuggestRequest,
                    document.uri.toString(),
                    document.version,
                    offset,
                    Date.now(),
                );

                if (autoSuggestMatched) {
                    pendingAutoSuggestRequest = undefined;
                }

                if (
                    shouldHideAutoTriggeredSuggest(
                        result,
                        context.triggerKind,
                        context.triggerCharacter,
                        autoSuggestMatched,
                    )
                ) {
                    void vscode.commands.executeCommand("hideSuggestWidget");
                }

                return result;
            },
            handleDiagnostics: (uri, diagnostics, next) => {
                diagnosticBuffer?.schedule(
                    uri.toString(),
                    { uri, diagnostics },
                    ({ uri: nextUri, diagnostics: nextDiagnostics }) => {
                        next(nextUri, [...nextDiagnostics]);
                    },
                );
            },
        },
        synchronize: {
            configurationSection: "kern",
            fileEvents: fileWatchers,
        },
    };

    client = new LanguageClient(
        "kern-lsp",
        "Kern Language Server",
        serverOptions,
        clientOptions,
    );

    client.onDidChangeState((event) => {
        if (event.newState === State.Running) {
            setStatus(
                `source=${server.source}`,
                "kern-lsp running",
                vscode.LanguageStatusSeverity.Information,
            );
            appendOutput(`kern-lsp is running (${server.source}).`);
        } else if (event.newState === State.Starting) {
            setStatus("starting", "Starting kern-lsp", vscode.LanguageStatusSeverity.Information);
        } else {
            setStatus("stopped", "kern-lsp stopped", vscode.LanguageStatusSeverity.Warning);
            appendOutput("kern-lsp stopped.");
        }
    });

    try {
        await client.start();
    } catch (error) {
        appendOutput(`Failed to start kern-lsp: ${formatError(error)}`);
        setStatus("failed", "kern-lsp failed to start", vscode.LanguageStatusSeverity.Error);
        void vscode.window.showErrorMessage(
            `Failed to start kern-lsp from ${server.source}. See the Kern Language Server output for details.`,
        );
        await stopLanguageServer();
    }
}

async function stopLanguageServer(): Promise<void> {
    cancelPendingAutoSuggest();
    autoSuggestDocuments.clear();
    disposeWatchers();
    diagnosticBuffer?.clear();
    diagnosticBuffer = undefined;
    if (!client) {
        return;
    }

    const current = client;
    client = undefined;
    await current.stop();
}

function appendOutput(message: string): void {
    outputChannel?.appendLine(`[kern] ${message}`);
}

function cancelPendingAutoSuggest(): void {
    if (pendingAutoSuggestTimer) {
        clearTimeout(pendingAutoSuggestTimer);
        pendingAutoSuggestTimer = undefined;
    }
    pendingAutoSuggestRequest = undefined;
}

function workspaceRoots(): WorkspaceRoot[] {
    return (vscode.workspace.workspaceFolders ?? []).map((folder) => ({
        fsPath: folder.uri.fsPath,
        name: folder.name,
    }));
}

function projectAnalysisArgs(config: vscode.WorkspaceConfiguration): string[] {
    const args: string[] = [];
    const features = config.get<string[]>("project.features", []);
    const normalized = features
        .map((feature) => feature.trim())
        .filter((feature) => feature.length > 0);
    if (normalized.length > 0) {
        args.push("--features", normalized.join(","));
    }
    if (config.get<boolean>("project.noDefaultFeatures", false)) {
        args.push("--no-default-features");
    }
    return args;
}

function configuredServerEnv(config: vscode.WorkspaceConfiguration): Record<string, string> {
    const raw = config.get<unknown>("server.env", {});
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
        return {};
    }

    const env: Record<string, string> = {};
    for (const [key, value] of Object.entries(raw)) {
        if (typeof value === "string") {
            env[key] = value;
        }
    }
    return env;
}

async function refreshCraftAnalysisContext(
    context: vscode.ExtensionContext,
): Promise<void> {
    const config = vscode.workspace.getConfiguration("kern");
    const roots = discoverCraftWorkspaceFolders(workspaceRoots(), fs.existsSync);
    if (roots.length === 0) {
        void vscode.window.showWarningMessage(
            "No workspace folder with Craft.toml was found.",
        );
        return;
    }

    const command = resolveCraftCommand(config.get<string>("craft.path", ""), roots[0]?.fsPath);
    const args = craftRefreshArgs(
        config.get<string[]>("project.features", []),
        config.get<boolean>("project.noDefaultFeatures", false),
    );
    const env = {
        ...process.env,
        ...configuredServerEnv(config),
    };

    outputChannel?.show(true);
    setStatus(
        "refreshing",
        "Refreshing craft analysis context",
        vscode.LanguageStatusSeverity.Information,
    );

    try {
        for (const root of roots) {
            appendOutput(
                `Refreshing craft analysis context in ${root.fsPath}: ${command} ${args.join(" ")}`,
            );
            await runCraftCommand(command, args, root.fsPath, env);
        }
        appendOutput("Craft analysis context refreshed.");
        await restartLanguageServer(context, false);
        void vscode.window.showInformationMessage(
            "Craft analysis context refreshed.",
        );
    } catch (error) {
        appendOutput(`Craft analysis context refresh failed: ${formatError(error)}`);
        setStatus(
            "refresh-failed",
            "Craft analysis refresh failed",
            vscode.LanguageStatusSeverity.Error,
        );
        void vscode.window.showErrorMessage(
            "Failed to refresh Craft analysis context. See the Kern Language Server output for details.",
        );
    }
}

async function runCraftTargetCommand(
    mode: "build" | "test",
    rawArgs: unknown,
): Promise<void> {
    const args =
        mode === "build"
            ? parseCraftBuildPackageArgs(rawArgs)
            : parseCraftTestTargetArgs(rawArgs);
    if (!args.manifestPath) {
        void vscode.window.showErrorMessage("Missing Craft manifest path for code lens command.");
        return;
    }

    const cwd = manifestWorkingDirectory(args.manifestPath);
    const config = vscode.workspace.getConfiguration("kern");
    const command = resolveCraftCommand(config.get<string>("craft.path", ""), cwd);
    const craftArgs = [
        mode,
        "--color=always",
        ...projectAnalysisArgs(config),
        "--project-path",
        args.manifestPath,
    ];
    if (mode === "test") {
        const targetName = (args as CraftTestTargetArgs).targetName;
        if (!targetName) {
            void vscode.window.showErrorMessage("Missing Craft test target name.");
            return;
        }
        craftArgs.push("--test", targetName);
    }
    const env = {
        ...process.env,
        ...configuredServerEnv(config),
    };

    setStatus(
        `craft-${mode}`,
        mode === "build" ? "Running craft build" : "Running craft test",
        vscode.LanguageStatusSeverity.Information,
    );
    appendOutput(`Running ${command} ${craftArgs.join(" ")} in ${cwd}`);

    try {
        await runCraftTerminalCommand(
            command,
            craftArgs,
            cwd,
            env,
            mode === "build" ? "Kern Craft Build" : "Kern Craft Test",
        );
        setStatus(
            `craft-${mode}-complete`,
            mode === "build" ? "Craft build completed" : "Craft test completed",
            vscode.LanguageStatusSeverity.Information,
        );
    } catch (error) {
        appendOutput(`Craft ${mode} failed: ${formatError(error)}`);
        setStatus(
            `craft-${mode}-failed`,
            mode === "build" ? "Craft build failed" : "Craft test failed",
            vscode.LanguageStatusSeverity.Error,
        );
        void vscode.window.showErrorMessage(
            `Craft ${mode} failed. See the Kern Language Server output for details.`,
        );
    }
}

function parseCraftBuildPackageArgs(raw: unknown): CraftBuildPackageArgs {
    if (!raw || typeof raw !== "object") {
        return {};
    }
    const value = raw as Record<string, unknown>;
    return {
        manifestPath:
            typeof value.manifestPath === "string" ? value.manifestPath : undefined,
    };
}

function parseCraftTestTargetArgs(raw: unknown): CraftTestTargetArgs {
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

function manifestWorkingDirectory(manifestPath: string): string {
    const normalized = manifestPath.replace(/\\/g, "/");
    const slash = normalized.lastIndexOf("/");
    if (slash <= 0) {
        return ".";
    }
    return manifestPath.slice(0, slash);
}

function runCraftCommand(
    command: string,
    args: string[],
    cwd: string,
    env: NodeJS.ProcessEnv,
): Promise<void> {
    return new Promise((resolve, reject) => {
        const child = spawn(command, args, {
            cwd,
            env,
            stdio: ["ignore", "pipe", "pipe"],
        });
        child.stdout.on("data", (chunk: Buffer | string) => {
            appendOutput(String(chunk).trimEnd());
        });
        child.stderr.on("data", (chunk: Buffer | string) => {
            appendOutput(String(chunk).trimEnd());
        });
        child.on("error", reject);
        child.on("close", (code) => {
            if (code === 0) {
                resolve();
                return;
            }
            reject(new Error(`craft exited with status ${code ?? "unknown"}`));
        });
    });
}

function runCraftTerminalCommand(
    command: string,
    args: string[],
    cwd: string,
    env: NodeJS.ProcessEnv,
    name: string,
): Promise<void> {
    return new Promise((resolve, reject) => {
        const terminal = vscode.window.createTerminal({
            name: `${name} #${++nextCraftTerminalId}`,
            pty: new CraftTaskTerminal(command, args, cwd, env, resolve, reject),
        });
        terminal.show(true);
    });
}

class CraftTaskTerminal implements vscode.Pseudoterminal {
    private readonly writeEmitter = new vscode.EventEmitter<string>();
    readonly onDidWrite = this.writeEmitter.event;

    private child: ReturnType<typeof spawn> | undefined;
    private completed = false;
    private closedByUser = false;

    constructor(
        private readonly command: string,
        private readonly args: string[],
        private readonly cwd: string,
        private readonly env: NodeJS.ProcessEnv,
        private readonly resolve: () => void,
        private readonly reject: (error: Error) => void,
    ) {}

    open(): void {
        this.writeLine(`$ ${this.command} ${this.args.map(shellQuote).join(" ")}`);
        const child = spawn(this.command, this.args, {
            cwd: this.cwd,
            env: this.env,
            stdio: ["ignore", "pipe", "pipe"],
        });
        this.child = child;
        child.stdout.on("data", (chunk: Buffer | string) => {
            this.write(String(chunk));
        });
        child.stderr.on("data", (chunk: Buffer | string) => {
            this.write(String(chunk));
        });
        child.on("error", (error) => {
            if (this.completed) {
                return;
            }
            this.completed = true;
            this.writeLine(`craft failed to start: ${formatError(error)}`);
            this.reject(error instanceof Error ? error : new Error(String(error)));
        });
        child.on("close", (code) => {
            if (this.completed) {
                return;
            }
            this.completed = true;
            if (this.closedByUser) {
                this.writeLine("[canceled] craft task stopped");
                this.reject(new Error("craft task was canceled"));
                return;
            }
            if (code === 0) {
                this.writeLine("[ok] craft completed");
                this.resolve();
                return;
            }
            const status = code ?? "unknown";
            this.writeLine(`[error] craft exited with status ${status}`);
            this.reject(new Error(`craft exited with status ${status}`));
        });
    }

    close(): void {
        if (this.completed) {
            return;
        }
        this.closedByUser = true;
        const child = this.child;
        if (child) {
            child.kill();
            return;
        }
        this.completed = true;
        this.reject(new Error("craft task was canceled"));
    }

    private write(output: string): void {
        this.writeEmitter.fire(output.replace(/\r?\n/g, "\r\n"));
    }

    private writeLine(output: string): void {
        this.write(`${output}\n`);
    }
}

function shellQuote(value: string): string {
    if (/^[A-Za-z0-9_./:=+-]+$/.test(value)) {
        return value;
    }
    return `'${value.replace(/'/g, "'\\''")}'`;
}

function createLanguageServerWatchers(): vscode.FileSystemWatcher[] {
    return [
        vscode.workspace.createFileSystemWatcher("**/*.kn"),
        vscode.workspace.createFileSystemWatcher("**/Craft.toml"),
        vscode.workspace.createFileSystemWatcher("**/.craft/analysis.toml"),
        vscode.workspace.createFileSystemWatcher("**/build.kn"),
    ];
}

function disposeWatchers(): void {
    for (const watcher of fileWatchers) {
        watcher.dispose();
    }
    fileWatchers = [];
}

function formatError(error: unknown): string {
    if (error instanceof Error) {
        return error.message;
    }
    return String(error);
}

function setStatus(
    detail: string,
    text: string,
    severity: vscode.LanguageStatusSeverity,
): void {
    if (!statusItem) {
        return;
    }

    statusItem.detail = detail;
    statusItem.text = text;
    statusItem.severity = severity;
    statusItem.busy = detail.includes("start") || detail.includes("restart");
}
