import * as fs from "node:fs";
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
import { shouldAutoTriggerSuggest } from "./clientBehavior";

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel | undefined;
let statusItem: vscode.LanguageStatusItem | undefined;
let fileWatchers: vscode.FileSystemWatcher[] = [];

type WorkspaceRoot = {
    fsPath: string;
    name: string;
};

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    outputChannel = vscode.window.createOutputChannel("Kern Language Server");
    statusItem = vscode.languages.createLanguageStatusItem("kern.lsp.status", {
        language: "kern",
        scheme: "file",
    });
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
        vscode.workspace.onDidChangeConfiguration((event) => {
            if (
                event.affectsConfiguration("kern.server") ||
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
                return;
            }

            const change = event.contentChanges[0];
            if (!shouldAutoTriggerSuggest(change.text, change.rangeLength ?? 0)) {
                return;
            }

            void vscode.commands.executeCommand("editor.action.triggerSuggest");
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
    const resolution = resolveServerCommand(
        {
            configuredPath: kernConfig.get<string>("server.path", ""),
            configuredArgs: serverArgs,
            workspaceRoots: workspaceRoots(),
            extensionPath: context.extensionPath,
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
    appendOutput(
        `Starting kern-lsp (${server.source}): ${server.command} ${server.args.join(" ")}`.trim(),
    );
    setStatus("starting", "Starting kern-lsp", vscode.LanguageStatusSeverity.Information);

    const serverEnv = {
        ...process.env,
        ...configuredServerEnv(kernConfig),
    };
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
        documentSelector: [{ scheme: "file", language: "kern" }],
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
    disposeWatchers();
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
            await runCraftCheck(command, args, root.fsPath, env);
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

function runCraftCheck(
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

function createLanguageServerWatchers(): vscode.FileSystemWatcher[] {
    return [
        vscode.workspace.createFileSystemWatcher("**/*.rn"),
        vscode.workspace.createFileSystemWatcher("**/Craft.toml"),
        vscode.workspace.createFileSystemWatcher("**/.craft/analysis.toml"),
        vscode.workspace.createFileSystemWatcher("**/craft.rn"),
        vscode.workspace.createFileSystemWatcher("**/build.rn"),
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
