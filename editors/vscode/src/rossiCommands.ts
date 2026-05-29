import {
    CancellationToken,
    Diagnostic,
    DiagnosticCollection,
    DiagnosticSeverity,
    ExtensionContext,
    OutputChannel,
    Position,
    ProgressLocation,
    Range,
    Uri,
    window,
    workspace,
    commands as vscodeCommands,
} from 'vscode';
import { spawn } from 'child_process';
import * as fs from 'fs/promises';
import * as os from 'os';
import * as path from 'path';

interface RossiRunResult {
    stdout: string;
    stderr: string;
    exitCode: number | null;
}

interface RossiRunOptions {
    title: string;
    cwd?: string;
    allowNonZeroExit?: boolean;
}

interface ValidationResult {
    file: string;
    success: boolean;
    inner_filename?: string;
    error?: string;
    component_type?: string;
    component_name?: string;
    severity?: string;
    rule_id?: string;
    origin?: string;
}

interface ValidationTarget {
    inputs: string[];
    cwd: string;
}

type InputKind =
    | 'eventbFile'
    | 'eventbDirectory'
    | 'rodinZip'
    | 'rodinXmlFile'
    | 'rodinXmlDirectory';

export class RossiCommandController {
    private readonly diagnostics: DiagnosticCollection;
    private readonly output: OutputChannel;
    private readonly waitForLanguageServer?: () => Promise<void>;

    constructor(
        diagnostics: DiagnosticCollection,
        output: OutputChannel,
        waitForLanguageServer?: () => Promise<void>
    ) {
        this.diagnostics = diagnostics;
        this.output = output;
        this.waitForLanguageServer = waitForLanguageServer;
    }

    async importRodinProject(uri?: Uri): Promise<void> {
        const input = uri?.fsPath ?? await this.pickInput('Import Rodin Project', ['zip', 'buc', 'bum'], true);
        if (!input) {
            return;
        }

        const outDir = await this.pickOutputDirectory('Select Import Output Folder');
        if (!outDir) {
            return;
        }

        await this.runAndReport(
            ['import', input, '-o', outDir],
            { title: 'Importing Rodin project' },
            `Imported Rodin project to ${outDir}`
        );
    }

    async exportCurrentFileToRodinZip(uri?: Uri): Promise<void> {
        const input = await this.getEventBFile(uri);
        if (!input) {
            window.showErrorMessage('Open or select a .eventb file to export.');
            return;
        }

        await this.saveDocumentIfOpen(input);

        const outZip = await this.pickZipOutput(input, '.zip');
        if (!outZip) {
            return;
        }

        await this.runAndReport(
            ['export', input, '-o', outZip],
            { title: 'Exporting Rodin ZIP' },
            `Exported Rodin ZIP to ${outZip}`
        );
    }

    async exportWorkspaceToRodinZip(): Promise<void> {
        const folder = await this.pickWorkspaceFolder();
        if (!folder) {
            return;
        }
        await this.saveOpenEventBDocumentsUnder(folder);

        const outZip = await this.pickZipOutput(folder, '.zip');
        if (!outZip) {
            return;
        }

        // `rossi export` walks the directory itself, so hand it the folder
        // directly instead of enumerating .eventb files here.
        await this.runAndReport(
            ['export', folder, '-o', outZip],
            { title: 'Exporting workspace to Rodin ZIP' },
            `Exported workspace to ${outZip}`
        );
    }

    async buildCheckedRodinZip(uri?: Uri): Promise<void> {
        const input = uri?.fsPath ?? await this.pickInput('Build Checked Rodin ZIP', ['eventb', 'txt', 'zip', 'buc', 'bum'], true);
        if (!input) {
            return;
        }

        const outZip = await this.pickZipOutput(input, '.checked.zip');
        if (!outZip) {
            return;
        }

        const kind = await classifyInput(input);

        if (kind === 'eventbFile' || kind === 'eventbDirectory') {
            await this.saveOpenEventBDocumentsUnder(input);
            await withTempDir(async (tmp) => {
                const tempZip = path.join(tmp, 'source.zip');
                await this.runRossi(['export', input, '-o', tempZip], {
                    title: 'Preparing Rodin source ZIP',
                });
                await this.runBuildAndReport(tempZip, outZip);
            });
            return;
        }

        if (kind === 'rodinXmlFile') {
            await this.runBuildAndReport(path.dirname(input), outZip);
            return;
        }

        await this.runBuildAndReport(input, outZip);
    }

    async validateCurrentFile(uri?: Uri): Promise<void> {
        const input = await this.getEventBFile(uri);
        if (!input) {
            window.showErrorMessage('Open or select a .eventb file to validate.');
            return;
        }

        await this.saveDocumentIfOpen(input);
        await this.validateInput(input);
    }

    async validateWorkspace(): Promise<void> {
        const folder = await this.pickWorkspaceFolder();
        if (!folder) {
            return;
        }
        await this.saveOpenEventBDocumentsUnder(folder);
        await this.validateInput(folder);
    }

    async convertCurrentFileToUnicode(uri?: Uri): Promise<void> {
        await this.convertCurrentFile(uri, false);
    }

    async convertCurrentFileToAscii(uri?: Uri): Promise<void> {
        await this.convertCurrentFile(uri, true);
    }

    async animateWithProb(uri?: Uri): Promise<void> {
        await this.executeProbCommand('rossi.prob.animate', uri);
    }

    async modelCheckWithProb(uri?: Uri): Promise<void> {
        await this.executeProbCommand('rossi.prob.modelcheck', uri);
    }

    async checkToolchain(): Promise<void> {
        try {
            const version = await this.runRossi(['--version'], {
                title: 'Checking Rossi tool',
            });
            await this.runRossi(['import', '--help'], {
                title: 'Checking Rossi import command',
            });
            await this.runRossi(['export', '--help'], {
                title: 'Checking Rossi export command',
            });
            await this.runRossi(['fmt', '--help'], {
                title: 'Checking Rossi fmt command',
            });
            await this.runRossi(['build', '--help'], {
                title: 'Checking Rossi build command',
            });
            await this.runRossi(['validate', '--help'], {
                title: 'Checking Rossi validate command',
            });

            const summary = firstNonEmptyLine(version.stdout) ?? 'rossi command is available';
            window.showInformationMessage(`Rossi toolchain OK: ${summary}`);
        } catch (error) {
            this.showCommandError(error);
        }
    }

    private async validateInput(input: string): Promise<void> {
        let target: ValidationTarget;
        try {
            target = await validationTargetFor(input);
        } catch (error) {
            this.showCommandError(error);
            return;
        }

        let result: RossiRunResult;
        try {
            result = await this.runRossi(
                ['validate', '--format', 'json', '--continue-on-error', ...target.inputs],
                {
                    title: 'Validating Event-B model',
                    cwd: target.cwd,
                    allowNonZeroExit: true,
                }
            );
        } catch (error) {
            this.showCommandError(error);
            return;
        }

        this.applyValidationDiagnostics(result.stdout, target.cwd);

        if (result.exitCode === 0) {
            window.showInformationMessage('Rossi validation completed.');
        } else {
            window.showWarningMessage('Rossi validation found issues. See Problems and Rossi output.');
        }
    }

    private async convertCurrentFile(uri: Uri | undefined, ascii: boolean): Promise<void> {
        const input = await this.getEventBFile(uri);
        if (!input) {
            window.showErrorMessage('Open or select a .eventb file to convert.');
            return;
        }

        await this.saveDocumentIfOpen(input);

        try {
            // `fmt` reformats in place across the same representation, so it
            // converts the operator convention directly — no Rodin round-trip.
            const result = await this.runRossi(
                ['fmt', input, ascii ? '--ascii' : '--unicode'],
                {
                    title: ascii ? 'Converting to ASCII' : 'Converting to Unicode',
                    cwd: path.dirname(input),
                }
            );
            await this.replaceDocumentText(input, result.stdout);
            window.showInformationMessage(`Converted ${path.basename(input)} to ${ascii ? 'ASCII' : 'Unicode'}.`);
        } catch (error) {
            this.showCommandError(error);
        }
    }

    private async executeProbCommand(command: string, uri?: Uri): Promise<void> {
        const input = await this.getEventBFile(uri);
        if (!input) {
            window.showErrorMessage('Open or select a .eventb file to run ProB.');
            return;
        }

        await this.saveDocumentIfOpen(input);

        try {
            await this.waitForLanguageServer?.();
            await vscodeCommands.executeCommand(command, Uri.file(input).toString());
        } catch (error) {
            this.showCommandError(error);
        }
    }

    private applyValidationDiagnostics(stdout: string, cwd: string): void {
        let rows: ValidationResult[];
        try {
            rows = JSON.parse(stdout) as ValidationResult[];
        } catch (error) {
            this.output.show(true);
            window.showErrorMessage(`Failed to parse rossi validation JSON: ${error}`);
            return;
        }

        const byUri = new Map<string, Diagnostic[]>();
        for (const row of rows) {
            if (!row.error && !row.severity) {
                continue;
            }

            const target = validationDiagnosticPath(row, cwd);
            const uri = Uri.file(target);
            const message = validationMessage(row);
            const diagnostic = new Diagnostic(
                new Range(new Position(0, 0), new Position(0, 1)),
                message,
                diagnosticSeverity(row.severity)
            );
            diagnostic.source = 'rossi';
            if (row.rule_id) {
                diagnostic.code = row.rule_id;
            }

            const key = uri.toString();
            const existing = byUri.get(key) ?? [];
            existing.push(diagnostic);
            byUri.set(key, existing);
        }

        this.diagnostics.clear();
        for (const [uri, diagnostics] of byUri.entries()) {
            this.diagnostics.set(Uri.parse(uri), diagnostics);
        }
    }

    private async runAndReport(args: string[], options: RossiRunOptions, successMessage: string): Promise<void> {
        try {
            await this.runRossi(args, options);
            window.showInformationMessage(successMessage);
        } catch (error) {
            this.showCommandError(error);
        }
    }

    private async runBuildAndReport(input: string, outZip: string): Promise<void> {
        try {
            const result = await this.runRossi(
                ['build', input, '-o', outZip],
                { title: 'Building checked Rodin ZIP' }
            );
            const errors = countBuildErrorDiagnostics(result);
            if (errors > 0) {
                window.showWarningMessage(
                    `Built checked Rodin ZIP with ${errors} error diagnostic(s). See Rossi output.`
                );
            } else {
                window.showInformationMessage(`Built checked Rodin ZIP at ${outZip}`);
            }
        } catch (error) {
            this.showCommandError(error);
        }
    }

    private async runRossi(args: string[], options: RossiRunOptions): Promise<RossiRunResult> {
        const toolPath = workspace.getConfiguration('rossi').get<string>('tool.path', 'rossi');
        const cwd = options.cwd ?? workspace.workspaceFolders?.[0]?.uri.fsPath;
        const commandLine = formatCommand(toolPath, args);

        this.output.appendLine(`> ${commandLine}`);

        return window.withProgress(
            {
                location: ProgressLocation.Notification,
                title: options.title,
                cancellable: true,
            },
            (_progress, token) => this.spawnRossi(toolPath, args, cwd, options.allowNonZeroExit ?? false, token)
        );
    }

    private spawnRossi(
        toolPath: string,
        args: string[],
        cwd: string | undefined,
        allowNonZeroExit: boolean,
        token: CancellationToken
    ): Promise<RossiRunResult> {
        return new Promise((resolve, reject) => {
            const child = spawn(toolPath, args, { cwd, shell: false });
            let stdout = '';
            let stderr = '';
            let settled = false;

            const finishReject = (error: Error) => {
                if (!settled) {
                    settled = true;
                    reject(error);
                }
            };

            const cancellation = token.onCancellationRequested(() => {
                child.kill();
                finishReject(new Error('Rossi command cancelled.'));
            });

            child.stdout.on('data', (data: Buffer) => {
                const text = data.toString();
                stdout += text;
                this.output.append(text);
            });

            child.stderr.on('data', (data: Buffer) => {
                const text = data.toString();
                stderr += text;
                this.output.append(text);
            });

            child.on('error', (error) => {
                cancellation.dispose();
                finishReject(new Error(`Failed to start '${toolPath}': ${error.message}`));
            });

            child.on('close', (code) => {
                cancellation.dispose();
                if (settled) {
                    return;
                }
                settled = true;
                this.output.appendLine('');
                if (code !== 0 && !allowNonZeroExit) {
                    reject(new Error(`Rossi command failed with exit code ${code}: ${formatCommand(toolPath, args)}`));
                    return;
                }
                resolve({ stdout, stderr, exitCode: code });
            });
        });
    }

    private showCommandError(error: unknown): void {
        this.output.show(true);
        const message = error instanceof Error ? error.message : String(error);
        window.showErrorMessage(message);
    }

    async runCommand(command: () => Promise<void>): Promise<void> {
        try {
            await command();
        } catch (error) {
            this.showCommandError(error);
        }
    }

    private async pickInput(title: string, extensions: string[], allowFolders: boolean): Promise<string | undefined> {
        const selection = await window.showOpenDialog({
            title,
            canSelectFiles: true,
            canSelectFolders: allowFolders,
            canSelectMany: false,
            filters: {
                'Supported Rossi Inputs': extensions,
            },
        });
        return selection?.[0]?.fsPath;
    }

    private async pickOutputDirectory(title: string): Promise<string | undefined> {
        const selection = await window.showOpenDialog({
            title,
            canSelectFiles: false,
            canSelectFolders: true,
            canSelectMany: false,
        });
        return selection?.[0]?.fsPath;
    }

    private async pickZipOutput(input: string, suffix: string): Promise<string | undefined> {
        const defaultPath = path.join(path.dirname(input), `${path.basename(input, path.extname(input))}${suffix}`);
        const selection = await window.showSaveDialog({
            defaultUri: Uri.file(defaultPath),
            filters: {
                'Rodin ZIP': ['zip'],
            },
        });
        return selection ? ensureZipExtension(selection.fsPath) : undefined;
    }

    private async pickWorkspaceFolder(): Promise<string | undefined> {
        const folders = workspace.workspaceFolders;
        if (!folders || folders.length === 0) {
            window.showErrorMessage('Open a workspace folder first.');
            return undefined;
        }
        if (folders.length === 1) {
            return folders[0].uri.fsPath;
        }

        const selected = await window.showQuickPick(
            folders.map((folder) => ({
                label: folder.name,
                description: folder.uri.fsPath,
                folder,
            })),
            { title: 'Select Workspace Folder' }
        );
        return selected?.folder.uri.fsPath;
    }

    private async getEventBFile(uri?: Uri): Promise<string | undefined> {
        if (uri?.scheme === 'file' && isEventBTextFile(uri.fsPath)) {
            return uri.fsPath;
        }

        const editor = window.activeTextEditor;
        if (editor?.document.uri.scheme === 'file' && isEventBTextFile(editor.document.uri.fsPath)) {
            return editor.document.uri.fsPath;
        }

        return undefined;
    }

    private async saveDocumentIfOpen(filePath: string): Promise<void> {
        const document = workspace.textDocuments.find((item) => item.uri.fsPath === filePath);
        if (document?.isDirty) {
            await document.save();
        }
    }

    private async saveOpenEventBDocumentsUnder(root: string): Promise<void> {
        const stats = await fs.stat(root).catch(() => undefined);
        const rootDir = stats?.isDirectory() ? root : path.dirname(root);
        for (const document of workspace.textDocuments) {
            if (
                document.uri.scheme === 'file' &&
                document.isDirty &&
                isEventBTextFile(document.uri.fsPath) &&
                isPathInside(document.uri.fsPath, rootDir)
            ) {
                await document.save();
            }
        }
    }

    private async replaceDocumentText(filePath: string, text: string): Promise<void> {
        const document = await workspace.openTextDocument(Uri.file(filePath));
        const editor = window.visibleTextEditors.find((item) => item.document.uri.fsPath === filePath)
            ?? await window.showTextDocument(document, { preview: false });
        const fullRange = new Range(
            document.positionAt(0),
            document.positionAt(document.getText().length)
        );
        const applied = await editor.edit((builder) => {
            builder.replace(fullRange, text);
        });
        if (!applied) {
            throw new Error(`Failed to update ${filePath}`);
        }
        await document.save();
    }
}

export function registerRossiCommands(
    context: ExtensionContext,
    diagnostics: DiagnosticCollection,
    output: OutputChannel,
    waitForLanguageServer?: () => Promise<void>
): void {
    const controller = new RossiCommandController(diagnostics, output, waitForLanguageServer);
    context.subscriptions.push(
        vscodeCommands.registerCommand('rossi.importRodinProject', (uri?: Uri) => controller.runCommand(() => controller.importRodinProject(uri))),
        vscodeCommands.registerCommand('rossi.exportCurrentFileToRodinZip', (uri?: Uri) => controller.runCommand(() => controller.exportCurrentFileToRodinZip(uri))),
        vscodeCommands.registerCommand('rossi.exportWorkspaceToRodinZip', () => controller.runCommand(() => controller.exportWorkspaceToRodinZip())),
        vscodeCommands.registerCommand('rossi.buildCheckedRodinZip', (uri?: Uri) => controller.runCommand(() => controller.buildCheckedRodinZip(uri))),
        vscodeCommands.registerCommand('rossi.validateCurrentFile', (uri?: Uri) => controller.runCommand(() => controller.validateCurrentFile(uri))),
        vscodeCommands.registerCommand('rossi.validateWorkspace', () => controller.runCommand(() => controller.validateWorkspace())),
        vscodeCommands.registerCommand('rossi.convertCurrentFileToUnicode', (uri?: Uri) => controller.runCommand(() => controller.convertCurrentFileToUnicode(uri))),
        vscodeCommands.registerCommand('rossi.convertCurrentFileToAscii', (uri?: Uri) => controller.runCommand(() => controller.convertCurrentFileToAscii(uri))),
        vscodeCommands.registerCommand('rossi.animateWithProb', (uri?: Uri) => controller.runCommand(() => controller.animateWithProb(uri))),
        vscodeCommands.registerCommand('rossi.modelCheckWithProb', (uri?: Uri) => controller.runCommand(() => controller.modelCheckWithProb(uri))),
        vscodeCommands.registerCommand('rossi.checkToolchain', () => controller.runCommand(() => controller.checkToolchain()))
    );
}

async function classifyInput(input: string): Promise<InputKind> {
    const stats = await fs.stat(input);
    if (stats.isDirectory()) {
        const kinds = await scanDirectory(input);
        if (kinds.hasEventB && kinds.hasRodinXml) {
            throw new Error(`Directory mixes .eventb/.txt and .buc/.bum files: ${input}`);
        }
        if (kinds.hasEventB) {
            return 'eventbDirectory';
        }
        if (kinds.hasRodinXml) {
            return 'rodinXmlDirectory';
        }
        throw new Error(`Directory contains no .eventb, .txt, .buc, or .bum files: ${input}`);
    }

    const ext = path.extname(input).toLowerCase();
    if (ext === '.eventb' || ext === '.txt') {
        return 'eventbFile';
    }
    if (ext === '.zip') {
        return 'rodinZip';
    }
    if (ext === '.buc' || ext === '.bum') {
        return 'rodinXmlFile';
    }
    throw new Error(`Unsupported input type: ${input}`);
}

async function validationTargetFor(input: string): Promise<ValidationTarget> {
    const kind = await classifyInput(input);
    if (kind === 'eventbDirectory') {
        const files = await collectEventBTextFiles(input);
        if (files.length === 0) {
            throw new Error(`Directory contains no .eventb or .txt files: ${input}`);
        }
        return { inputs: files, cwd: input };
    }

    if (kind === 'rodinXmlDirectory') {
        return { inputs: [input], cwd: input };
    }

    return { inputs: [input], cwd: path.dirname(input) };
}

async function scanDirectory(dir: string): Promise<{ hasEventB: boolean; hasRodinXml: boolean }> {
    const result = { hasEventB: false, hasRodinXml: false };
    const entries = await fs.readdir(dir, { withFileTypes: true });
    for (const entry of entries) {
        const entryPath = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            const child = await scanDirectory(entryPath);
            result.hasEventB ||= child.hasEventB;
            result.hasRodinXml ||= child.hasRodinXml;
        } else if (entry.isFile()) {
            const ext = path.extname(entry.name).toLowerCase();
            result.hasEventB ||= ext === '.eventb' || ext === '.txt';
            result.hasRodinXml ||= ext === '.buc' || ext === '.bum';
        }
    }
    return result;
}

async function collectEventBTextFiles(dir: string): Promise<string[]> {
    const files: string[] = [];
    const entries = await fs.readdir(dir, { withFileTypes: true });
    for (const entry of entries) {
        const entryPath = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            files.push(...await collectEventBTextFiles(entryPath));
        } else if (entry.isFile()) {
            const ext = path.extname(entry.name).toLowerCase();
            if (ext === '.eventb' || ext === '.txt') {
                files.push(entryPath);
            }
        }
    }
    files.sort();
    return files;
}

async function withTempDir<T>(callback: (dir: string) => Promise<T>): Promise<T> {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'rossi-vscode-'));
    try {
        return await callback(dir);
    } finally {
        await fs.rm(dir, { recursive: true, force: true });
    }
}

function validationDiagnosticPath(row: ValidationResult, cwd: string): string {
    const target = path.isAbsolute(row.file) ? row.file : path.resolve(cwd, row.file);
    if (row.inner_filename && path.extname(target).toLowerCase() !== '.zip') {
        return path.join(target, row.inner_filename);
    }
    return target;
}

function countBuildErrorDiagnostics(result: RossiRunResult): number {
    const output = `${result.stdout}\n${result.stderr}`;
    const summary = output.match(/\((\d+) error diagnostic\(s\)\)/);
    if (summary) {
        return Number.parseInt(summary[1], 10);
    }
    return output.split(/\r?\n/).filter((line) => line.startsWith('[error]')).length;
}

function validationMessage(row: ValidationResult): string {
    const parts = [];
    if (row.rule_id) {
        parts.push(`[${row.rule_id}]`);
    }
    if (row.inner_filename) {
        parts.push(`${row.inner_filename}:`);
    }
    if (row.origin) {
        parts.push(`${row.origin}:`);
    }
    parts.push(row.error ?? row.severity ?? 'Validation issue');
    return parts.join(' ');
}

function diagnosticSeverity(severity: string | undefined): DiagnosticSeverity {
    switch (severity) {
        case 'warning':
            return DiagnosticSeverity.Warning;
        case 'info':
            return DiagnosticSeverity.Information;
        case 'hint':
            return DiagnosticSeverity.Hint;
        default:
            return DiagnosticSeverity.Error;
    }
}

function formatCommand(command: string, args: string[]): string {
    return [command, ...args].map(quoteArg).join(' ');
}

function quoteArg(value: string): string {
    return /\s/.test(value) ? `"${value.replace(/"/g, '\\"')}"` : value;
}

function ensureZipExtension(filePath: string): string {
    return path.extname(filePath).toLowerCase() === '.zip' ? filePath : `${filePath}.zip`;
}

function isEventBTextFile(filePath: string): boolean {
    return path.extname(filePath).toLowerCase() === '.eventb';
}

function isPathInside(candidate: string, root: string): boolean {
    const relative = path.relative(root, candidate);
    return relative === '' || (!relative.startsWith('..') && !path.isAbsolute(relative));
}

function firstNonEmptyLine(text: string): string | undefined {
    return text.split(/\r?\n/).find((line) => line.trim().length > 0)?.trim();
}
