import { workspace, ExtensionContext, window, languages } from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    ExecutableOptions,
} from 'vscode-languageclient/node';
import { registerRossiCommands } from './rossiCommands';
import { registerSymbolInput } from './symbolInput';
import { resolveBinaries, ResolvedBinaries, pruneToolchainCache } from './binaryManager';

let client: LanguageClient;

interface RossiConfiguration {
    format: {
        style: string;
        useUnicode: boolean;
        enforceUnicode: boolean;
        indentation: string;
        keywordCase: string;
        declLists: string;
        blankBetweenClauses: boolean | null;
        maxLineWidth: number;
    };
    diagnostics: {
        enabled: boolean;
        debounceMs: number;
    };
    completion: {
        enabled: boolean;
    };
    inlayHints: {
        enabled: boolean;
        wellDefinedness: boolean;
        maxLength: number;
    };
    rodin: {
        path: string;
        workspace: string;
        sync: boolean;
        bridge: boolean;
        mirrorProofs: boolean;
    };
    animate: {
        path: string;
        timeLimitSecs: number;
        disproveTimeoutMs: number;
    };
}

function getRossiConfiguration(): RossiConfiguration {
    const config = workspace.getConfiguration('rossi');
    return {
        format: {
            style: config.get<string>('format.style', ''),
            useUnicode: config.get<boolean>('format.useUnicode', true),
            enforceUnicode: config.get<boolean>('format.enforceUnicode', false),
            indentation: config.get<string>('format.indentation', ''),
            keywordCase: config.get<string>('format.keywordCase', ''),
            declLists: config.get<string>('format.declLists', ''),
            blankBetweenClauses: config.get<boolean | null>('format.blankBetweenClauses', null),
            maxLineWidth: config.get<number>('format.maxLineWidth', 120),
        },
        diagnostics: {
            enabled: config.get<boolean>('diagnostics.enabled', true),
            debounceMs: config.get<number>('diagnostics.debounceMs', 500),
        },
        completion: {
            enabled: config.get<boolean>('completion.enabled', true),
        },
        inlayHints: {
            enabled: config.get<boolean>('inlayHints.enabled', true),
            wellDefinedness: config.get<boolean>('inlayHints.wellDefinedness', true),
            maxLength: config.get<number>('inlayHints.maxLength', 32),
        },
        rodin: {
            path: config.get<string>('rodin.path', ''),
            workspace: config.get<string>('rodin.workspace', ''),
            sync: config.get<boolean>('rodin.sync', true),
            bridge: config.get<boolean>('rodin.bridge', true),
            mirrorProofs: config.get<boolean>('rodin.mirrorProofs', true),
        },
        animate: {
            path: config.get<string>('animate.path', ''),
            timeLimitSecs: config.get<number>('animate.timeLimitSecs', 120),
            disproveTimeoutMs: config.get<number>('animate.disproveTimeoutMs', 1000),
        },
    };
}

export async function activate(context: ExtensionContext) {
    console.log('Event-B (Rossi) extension is now active');

    const diagnostics = languages.createDiagnosticCollection('rossi');
    const output = window.createOutputChannel('Rossi');
    context.subscriptions.push(diagnostics, output);

    const config = workspace.getConfiguration('rossi');

    // Garbage-collect superseded downloaded toolchains left in global storage by
    // earlier extension versions. Best-effort and independent of how the binaries
    // resolve below, so it must not block activation — fire and forget.
    void pruneToolchainCache(context, output);

    // Locate (and, if missing, download) the CLI and language-server binaries.
    // On failure, fall back to the bare command names so a developer with the
    // binaries on PATH still works and the error message guides everyone else.
    let binaries: ResolvedBinaries;
    try {
        binaries = await resolveBinaries(context, output);
    } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        window.showErrorMessage(
            `Rossi: could not obtain the Event-B toolchain (${message}). ` +
            'Falling back to PATH — see the extension\'s install guide to set it up manually.'
        );
        binaries = { languageServer: 'eventb-language-server', cli: 'rossi' };
    }

    // Configure server options
    const serverOptions: ServerOptions = {
        command: binaries.languageServer,
        args: [],
        options: <ExecutableOptions>{
            env: {
                ...process.env,
                RUST_LOG: config.get<string>('trace.server') === 'verbose' ? 'debug' : 'info',
            },
        },
    };

    // Configure client options
    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'eventb' }],
        synchronize: {
            configurationSection: 'rossi',
            // No `fileEvents` watcher here: the server registers its own
            // `**/*.eventb` watcher once it has started.
        },
        initializationOptions: getRossiConfiguration(),
    };

    // Create the language client
    client = new LanguageClient(
        'rossiLanguageServer',
        'Rossi Language Server',
        serverOptions,
        clientOptions
    );
    context.subscriptions.push(client);

    // Start the client (which will start the server)
    const languageServerReady = client.start().then(() => {
        console.log('Rossi Language Server started');
    }).catch((error) => {
        window.showErrorMessage(`Failed to start Rossi Language Server: ${error.message}`);
        console.error('Failed to start Rossi Language Server:', error);
        throw error;
    });
    languageServerReady.catch(() => undefined);
    registerRossiCommands(context, diagnostics, output, binaries.cli, () => languageServerReady);

    // Editor-side ASCII -> Unicode input method (type `=>`, `\and`, ...).
    registerSymbolInput(context, client, languageServerReady, output);
}

export function deactivate(): Thenable<void> | undefined {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
