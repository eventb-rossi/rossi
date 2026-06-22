import { workspace, ExtensionContext, window, languages } from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    ExecutableOptions,
} from 'vscode-languageclient/node';
import { registerRossiCommands } from './rossiCommands';
import { registerSymbolInput } from './symbolInput';
import { resolveBinaries, ResolvedBinaries } from './binaryManager';

let client: LanguageClient;

interface RossiConfiguration {
    format: {
        useUnicode: boolean;
        indentation: string;
        maxLineLength: number;
    };
    diagnostics: {
        enabled: boolean;
        debounceMs: number;
    };
    completion: {
        enabled: boolean;
    };
    trace: {
        server: string;
    };
}

function getRossiConfiguration(): RossiConfiguration {
    const config = workspace.getConfiguration('rossi');
    return {
        format: {
            useUnicode: config.get<boolean>('format.useUnicode', true),
            indentation: config.get<string>('format.indentation', '    '),
            maxLineLength: config.get<number>('format.maxLineLength', 100),
        },
        diagnostics: {
            enabled: config.get<boolean>('diagnostics.enabled', true),
            debounceMs: config.get<number>('diagnostics.debounceMs', 500),
        },
        completion: {
            enabled: config.get<boolean>('completion.enabled', true),
        },
        trace: {
            server: config.get<string>('trace.server', 'off'),
        },
    };
}

export async function activate(context: ExtensionContext) {
    console.log('Event-B (Rossi) extension is now active');

    const diagnostics = languages.createDiagnosticCollection('rossi');
    const output = window.createOutputChannel('Rossi');
    context.subscriptions.push(diagnostics, output);

    const config = workspace.getConfiguration('rossi');

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
            // Notify the server about file changes to '.eventb' files in the workspace
            fileEvents: workspace.createFileSystemWatcher('**/*.eventb'),
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
