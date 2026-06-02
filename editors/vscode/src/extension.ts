import { workspace, ExtensionContext, window, languages } from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    ExecutableOptions,
} from 'vscode-languageclient/node';
import { registerRossiCommands } from './rossiCommands';
import { registerSymbolInput } from './symbolInput';

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
    prob: {
        enabled: boolean;
        path: string;
        timeout: number;
        animateSteps: number;
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
        prob: {
            enabled: config.get<boolean>('prob.enabled', true),
            path: config.get<string>('prob.path', ''),
            timeout: config.get<number>('prob.timeout', 10000),
            animateSteps: config.get<number>('prob.animateSteps', 5),
        },
    };
}

export function activate(context: ExtensionContext) {
    console.log('Rossi Event-B extension is now active');

    const diagnostics = languages.createDiagnosticCollection('rossi');
    const output = window.createOutputChannel('Rossi');
    context.subscriptions.push(diagnostics, output);

    // Get configuration
    const config = workspace.getConfiguration('rossi');
    const serverPath = config.get<string>('languageServer.path', 'rossi-language-server');

    // Configure server options
    const serverOptions: ServerOptions = {
        command: serverPath,
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
    registerRossiCommands(context, diagnostics, output, () => languageServerReady);

    // Editor-side ASCII -> Unicode input method (type `=>`, `\and`, ...).
    registerSymbolInput(context, client, languageServerReady);
}

export function deactivate(): Thenable<void> | undefined {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
