import * as fs from "node:fs";
import * as vscode from "vscode";
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    State,
} from "vscode-languageclient/node";
import {
    resolveServerCommand,
    type ResolvedServerCommand,
} from "./serverResolution";

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel | undefined;
let statusItem: vscode.LanguageStatusItem | undefined;
let fileWatchers: vscode.FileSystemWatcher[] = [];

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
        vscode.workspace.onDidChangeConfiguration((event) => {
            if (event.affectsConfiguration("kern.server")) {
                void restartLanguageServer(context, false);
            }
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
    const resolution = resolveServerCommand(
        {
            configuredPath: vscode.workspace
                .getConfiguration("kern")
                .get<string>("server.path", ""),
            configuredArgs: vscode.workspace
                .getConfiguration("kern")
                .get<string[]>("server.args", []),
            workspaceRoots: (vscode.workspace.workspaceFolders ?? []).map((folder) => ({
                fsPath: folder.uri.fsPath,
                name: folder.name,
            })),
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

    const serverOptions: ServerOptions = {
        run: {
            command: server.command,
            args: server.args,
        },
        debug: {
            command: server.command,
            args: server.args,
        },
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: "file", language: "kern" }],
        outputChannel,
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

function createLanguageServerWatchers(): vscode.FileSystemWatcher[] {
    return [
        vscode.workspace.createFileSystemWatcher("**/*.rn"),
        vscode.workspace.createFileSystemWatcher("**/Craft.toml"),
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
