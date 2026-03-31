import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    context.subscriptions.push(
        vscode.commands.registerCommand("kern.restartLanguageServer", async () => {
            await restartLanguageServer(context, true);
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
    if (!client) {
        return;
    }

    const current = client;
    client = undefined;
    await current.stop();
}

async function restartLanguageServer(
    context: vscode.ExtensionContext,
    explicit: boolean,
): Promise<void> {
    if (client) {
        const current = client;
        client = undefined;
        await current.stop();
    }

    await startLanguageServer(context);

    if (explicit) {
        void vscode.window.showInformationMessage("Kern language server restarted.");
    }
}

async function startLanguageServer(context: vscode.ExtensionContext): Promise<void> {
    const server = resolveServerCommand();
    if (!server) {
        void vscode.window.showErrorMessage(
            "Unable to start kern-lsp. Configure `kern.server.path` or install `kern-lsp` on PATH.",
        );
        return;
    }

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
        outputChannelName: "Kern Language Server",
    };

    client = new LanguageClient(
        "kern-lsp",
        "Kern Language Server",
        serverOptions,
        clientOptions,
    );

    await client.start();
}

function resolveServerCommand(): ResolvedServerCommand | undefined {
    const config = vscode.workspace.getConfiguration("kern");
    const configuredPath = config.get<string>("server.path", "").trim();
    const configuredArgs = config.get<string[]>("server.args", []);

    if (configuredPath.length > 0) {
        if (looksLikeAbsoluteExecutablePath(configuredPath) && !fs.existsSync(configuredPath)) {
            void vscode.window.showErrorMessage(
                `Configured kern-lsp path does not exist: ${configuredPath}`,
            );
            return undefined;
        }

        return {
            command: configuredPath,
            args: configuredArgs,
        };
    }

    for (const folder of vscode.workspace.workspaceFolders ?? []) {
        for (const candidate of localServerCandidates(folder.uri.fsPath)) {
            if (fs.existsSync(candidate)) {
                return {
                    command: candidate,
                    args: configuredArgs,
                };
            }
        }
    }

    return {
        command: executableName("kern-lsp"),
        args: configuredArgs,
    };
}

function looksLikeAbsoluteExecutablePath(value: string): boolean {
    return path.isAbsolute(value) || value.includes(path.sep);
}

function localServerCandidates(root: string): string[] {
    const name = executableName("kern-lsp");
    return [
        path.join(root, "target", "debug", name),
        path.join(root, "target", "release", name),
    ];
}

function executableName(base: string): string {
    return process.platform === "win32" ? `${base}.exe` : base;
}

interface ResolvedServerCommand {
    command: string;
    args: string[];
}
