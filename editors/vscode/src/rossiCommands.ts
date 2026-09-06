import {
    CancellationToken,
    CancellationTokenSource,
    Diagnostic,
    DiagnosticCollection,
    DiagnosticSeverity,
    ExtensionContext,
    OutputChannel,
    Position,
    ProgressLocation,
    Range,
    TextDocument,
    Uri,
    window,
    workspace,
    commands as vscodeCommands,
} from 'vscode';
import { spawn } from 'child_process';
import * as fs from 'fs/promises';
import * as path from 'path';
import { resolveCommandCwd } from './commandCwd';
import { formatStyleFlags } from './styleFlags';
import { regionToZeroIndexed, ValidationRegion } from './validationRegion';

interface RossiRunResult {
    stdout: string;
    stderr: string;
    exitCode: number | null;
}

interface RossiRunOptions {
    title: string;
    /** `null` runs independently of the workspace from a stable temp directory. */
    cwd?: string | null;
    allowNonZeroExit?: boolean;
    /** Text piped to the child's standard input (for `-` / stdin inputs). */
    stdin?: string;
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
    region?: ValidationRegion;
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

// `rossi validate` invocation shared by the on-demand and on-save paths: emit
// machine-readable JSON and keep going past the first failing file so every
// input is reported.
const VALIDATE_JSON_ARGS = ['validate', '--format', 'json', '--continue-on-error'];

// Quiet window after a save before the on-save validation fires, so a burst of
// saves coalesces into one project re-check instead of one CLI run per file.
const ON_SAVE_DEBOUNCE_MS = 300;

export class RossiCommandController {
    private readonly diagnostics: DiagnosticCollection;
    private readonly output: OutputChannel;
    private readonly cliPath: string;
    private readonly waitForLanguageServer?: () => Promise<void>;
    /** Cancellation handle for the in-flight validate-on-save run, if any. */
    private onSaveRun?: CancellationTokenSource;

    constructor(
        diagnostics: DiagnosticCollection,
        output: OutputChannel,
        cliPath: string,
        waitForLanguageServer?: () => Promise<void>
    ) {
        this.diagnostics = diagnostics;
        this.output = output;
        this.cliPath = cliPath;
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

        const outZip = await this.pickZipOutput(input, '.zip');
        if (!outZip) {
            return;
        }

        // Pipe the in-editor buffer via stdin so unsaved edits export without
        // forcing a save to disk.
        const buffer = await this.readEventBBuffer(input);
        await this.runAndReport(
            ['export', '-', '-o', outZip],
            { title: 'Exporting Rodin ZIP', stdin: buffer },
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

        // `build` now reads .eventb/.txt files and directories of them directly,
        // so the old export → temp .zip → build round-trip is gone.
        if (kind === 'eventbFile' || kind === 'eventbDirectory') {
            await this.saveOpenEventBDocumentsUnder(input);
        }

        // A single .buc/.bum belongs to a Rodin project on disk; build the whole
        // project directory so sibling components resolve.
        const buildInput = kind === 'rodinXmlFile' ? path.dirname(input) : input;
        await this.runBuildAndReport(buildInput, outZip);
    }

    async validateCurrentFile(uri?: Uri): Promise<void> {
        const input = await this.getEventBFile(uri);
        if (!input) {
            window.showErrorMessage('Open or select a .eventb file to validate.');
            return;
        }

        // Validate the in-editor buffer via stdin so unsaved edits are checked
        // without forcing a save to disk; `--stdin-filename` maps the
        // diagnostics back to the document.
        const buffer = await this.readEventBBuffer(input);
        await this.runValidate(['--stdin-filename', input, '-'], path.dirname(input), buffer);
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
        await this.runValidate(target.inputs, target.cwd);
    }

    // Run `rossi validate --format json` over `inputs`, optionally feeding a
    // buffer via stdin, and surface the diagnostics.
    private async runValidate(inputs: string[], cwd: string, stdin?: string): Promise<void> {
        let result: RossiRunResult;
        try {
            result = await this.runRossi(
                [...VALIDATE_JSON_ARGS, ...inputs],
                {
                    title: 'Validating Event-B model',
                    cwd,
                    allowNonZeroExit: true,
                    stdin,
                }
            );
        } catch (error) {
            this.showCommandError(error);
            return;
        }

        this.applyValidationDiagnostics(result.stdout, cwd);

        if (result.exitCode === 0) {
            window.showInformationMessage('Rossi validation completed.');
        } else {
            window.showWarningMessage('Rossi validation found issues. See Problems and Rossi output.');
        }
    }

    // Validate the project a just-saved .eventb file belongs to and refresh the
    // diagnostics. Unlike `validateWorkspace`, this runs quietly: it spawns the
    // CLI directly (no progress notification) and shows no completion popups, so
    // an automatic pass never interrupts editing.
    //
    // The saved file's *directory* is handed to `rossi validate` as a single
    // argument so the CLI loads it as one project and runs the full static
    // checker across the components — that is what adds the type/dead-code
    // diagnostics (EB006/EB018/EB011-014) the live language server does not
    // compute. (A bare file argument, or a list of files, only gets the
    // component-local lints the server already provides.) Scoping to the file's
    // directory keeps unrelated projects from cross-contaminating the result and
    // avoids re-checking the whole tree on every save. This assumes a project's
    // components are colocated in one directory (as `rossi import`/New Project
    // produce); a component split into a sibling subdirectory would not see its
    // cross-referenced siblings here.
    async validateWorkspaceOnSave(document: TextDocument): Promise<void> {
        if (document.uri.scheme !== 'file' || !isEventBTextFile(document.uri.fsPath)) {
            return;
        }
        const projectDir = path.dirname(document.uri.fsPath);

        // A newer save supersedes any in-flight run; the latest save wins.
        this.onSaveRun?.cancel();
        const source = new CancellationTokenSource();
        this.onSaveRun = source;

        try {
            const toolPath = this.resolveToolPath();
            const args = [...VALIDATE_JSON_ARGS, projectDir];
            this.output.appendLine(`> ${formatCommand(toolPath, args)}`);
            const result = await this.spawnCommand(toolPath, args, projectDir, true, source.token);
            if (source.token.isCancellationRequested) {
                return;
            }
            this.applyValidationDiagnostics(result.stdout, projectDir, { quiet: true, scopeDir: projectDir });
        } catch (error) {
            // Superseded by a newer save: drop the stale run silently.
            if (source.token.isCancellationRequested) {
                return;
            }
            // A background on-save pass must never raise an error dialog; log only.
            const message = error instanceof Error ? error.message : String(error);
            this.output.appendLine(`Validate on save failed: ${message}`);
        } finally {
            if (this.onSaveRun === source) {
                this.onSaveRun = undefined;
            }
            source.dispose();
        }
    }

    private async convertCurrentFile(uri: Uri | undefined, ascii: boolean): Promise<void> {
        const input = await this.getEventBFile(uri);
        if (!input) {
            window.showErrorMessage('Open or select a .eventb file to convert.');
            return;
        }

        try {
            // `fmt` reformats across the same representation, converting the
            // operator convention directly — no Rodin round-trip. Feed the
            // in-editor buffer via stdin and write the result back, so unsaved
            // edits convert without forcing a save to disk. The user's
            // `rossi.format.*` settings ride along so the result matches what
            // the LSP formatter would produce.
            const buffer = await this.readEventBBuffer(input);
            const format = workspace.getConfiguration('rossi');
            const styleFlags = formatStyleFlags({
                style: format.get('format.style'),
                keywordCase: format.get('format.keywordCase'),
                declLists: format.get('format.declLists'),
                blankBetweenClauses: format.get('format.blankBetweenClauses'),
                indentation: format.get('format.indentation'),
                maxLineWidth: format.get('format.maxLineWidth'),
                privateUseGlyphs: format.get('format.privateUseGlyphs'),
            });
            const result = await this.runRossi(
                ['fmt', '-', ascii ? '--ascii' : '--unicode', ...styleFlags],
                {
                    title: ascii ? 'Converting to ASCII' : 'Converting to Unicode',
                    cwd: null,
                    stdin: buffer,
                }
            );
            const saved = await this.replaceDocumentText(input, result.stdout);
            const style = ascii ? 'ASCII' : 'Unicode';
            if (!saved) {
                window.showWarningMessage(
                    `Converted ${path.basename(input)} to ${style} in the editor, but it could not be saved. ` +
                    'Use Save As to preserve the result.'
                );
            } else {
                window.showInformationMessage(`Converted ${path.basename(input)} to ${style}.`);
            }
        } catch (error) {
            this.showCommandError(error);
        }
    }

    private applyValidationDiagnostics(
        stdout: string,
        cwd: string,
        opts?: { quiet?: boolean; scopeDir?: string }
    ): void {
        let rows: ValidationResult[];
        try {
            rows = JSON.parse(stdout) as ValidationResult[];
        } catch (error) {
            if (opts?.quiet) {
                // A background pass must not pop a dialog or yank the output
                // channel into focus (e.g. when `rossi` crashes and writes no
                // JSON): record it quietly and leave existing diagnostics alone.
                this.output.appendLine(`Validate on save: could not parse rossi JSON output: ${error}`);
                return;
            }
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
            const r = regionToZeroIndexed(row.region);
            const diagnostic = new Diagnostic(
                new Range(
                    new Position(r.startLine, r.startChar),
                    new Position(r.endLine, r.endChar)
                ),
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

        const scopeDir = opts?.scopeDir;
        if (scopeDir) {
            // Scoped refresh: drop only this project's previous diagnostics so a
            // save in one project never erases another project's results.
            const stale: Uri[] = [];
            this.diagnostics.forEach((uri) => {
                if (isPathInside(uri.fsPath, scopeDir)) {
                    stale.push(uri);
                }
            });
            for (const uri of stale) {
                this.diagnostics.delete(uri);
            }
        } else {
            this.diagnostics.clear();
        }
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

    // An explicit `rossi.tool.path` override always wins and stays live (no
    // window reload needed); otherwise use the path resolved at activation
    // (a copy on PATH or the one downloaded into the extension's storage).
    private resolveToolPath(): string {
        const configured = workspace.getConfiguration('rossi').get<string>('tool.path', 'rossi').trim();
        return configured && configured !== 'rossi' ? configured : this.cliPath;
    }

    private async runRossi(args: string[], options: RossiRunOptions): Promise<RossiRunResult> {
        const toolPath = this.resolveToolPath();
        const cwd = resolveCommandCwd(options.cwd, workspace.workspaceFolders?.[0]?.uri.fsPath);
        const commandLine = formatCommand(toolPath, args);

        this.output.appendLine(`> ${commandLine}`);

        return window.withProgress(
            {
                location: ProgressLocation.Notification,
                title: options.title,
                cancellable: true,
            },
            (_progress, token) =>
                this.spawnCommand(toolPath, args, cwd, options.allowNonZeroExit ?? false, token, options.stdin)
        );
    }

    private spawnCommand(
        command: string,
        args: string[],
        cwd: string | undefined,
        allowNonZeroExit: boolean,
        token: CancellationToken,
        stdin?: string
    ): Promise<RossiRunResult> {
        return new Promise((resolve, reject) => {
            const child = spawn(command, args, { cwd, shell: false });
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
                finishReject(new Error('Command cancelled.'));
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
                finishReject(new Error(`Failed to start '${command}': ${error.message}`));
            });

            child.on('close', (code) => {
                cancellation.dispose();
                if (settled) {
                    return;
                }
                settled = true;
                this.output.appendLine('');
                if (code !== 0 && !allowNonZeroExit) {
                    reject(new Error(`Command failed with exit code ${code}: ${formatCommand(command, args)}`));
                    return;
                }
                resolve({ stdout, stderr, exitCode: code });
            });

            // Feed the buffer when piping a `-` input, otherwise close stdin so
            // the child never blocks waiting on input. Ignore EPIPE: if the
            // child exits before reading, its exit code/stderr is the real error.
            child.stdin.on('error', () => undefined);
            child.stdin.end(stdin ?? '');
        });
    }

    private showCommandError(error: unknown): void {
        this.output.show(true);
        const message = error instanceof Error ? error.message : String(error);
        window.showErrorMessage(message);
    }

    async newProject(): Promise<void> {
        const name = await window.showInputBox({
            title: 'New Event-B Project',
            prompt: 'Enter a name for your Event-B project',
            placeHolder: 'my_model',
            validateInput: (value) =>
                /^[A-Za-z]\w*$/.test(value)
                    ? undefined
                    : 'A project name must start with a letter and contain only letters, digits, or underscores.',
        });
        if (!name) {
            return;
        }

        const parent = await this.pickOutputDirectory('Select a folder to create the project in');
        if (!parent) {
            return;
        }

        const projectDir = path.join(parent, name);
        if (await pathExists(projectDir)) {
            window.showErrorMessage(`A folder named "${name}" already exists in ${parent}.`);
            return;
        }

        await fs.mkdir(projectDir, { recursive: true });
        await fs.writeFile(path.join(projectDir, `${name}_ctx.eventb`), starterContext(name), 'utf8');
        await fs.writeFile(path.join(projectDir, `${name}.eventb`), starterMachine(name), 'utf8');
        await fs.writeFile(path.join(projectDir, 'README.md'), starterReadme(name), 'utf8');
        await fs.writeFile(path.join(projectDir, '.gitignore'), STARTER_GITIGNORE, 'utf8');
        this.output.appendLine(`Created Event-B project at ${projectDir}`);

        // Open in a new window only when a workspace is already loaded, so the new
        // project does not replace the user's current session unexpectedly.
        const openInNewWindow = Boolean(workspace.workspaceFolders?.length);
        await vscodeCommands.executeCommand(
            'vscode.openFolder',
            Uri.file(projectDir),
            { forceNewWindow: openInNewWindow }
        );
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

    /** The in-editor text for `filePath` if it is open, else the file on disk. */
    private async readEventBBuffer(filePath: string): Promise<string> {
        const open = workspace.textDocuments.find((item) => item.uri.fsPath === filePath);
        return open ? open.getText() : fs.readFile(filePath, 'utf8');
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

    private async replaceDocumentText(filePath: string, text: string): Promise<boolean> {
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
        return document.save();
    }
}

export function registerRossiCommands(
    context: ExtensionContext,
    diagnostics: DiagnosticCollection,
    output: OutputChannel,
    cliPath: string,
    waitForLanguageServer?: () => Promise<void>
): void {
    const controller = new RossiCommandController(diagnostics, output, cliPath, waitForLanguageServer);
    context.subscriptions.push(
        vscodeCommands.registerCommand('rossi.importRodinProject', (uri?: Uri) => controller.runCommand(() => controller.importRodinProject(uri))),
        vscodeCommands.registerCommand('rossi.exportCurrentFileToRodinZip', (uri?: Uri) => controller.runCommand(() => controller.exportCurrentFileToRodinZip(uri))),
        vscodeCommands.registerCommand('rossi.exportWorkspaceToRodinZip', () => controller.runCommand(() => controller.exportWorkspaceToRodinZip())),
        vscodeCommands.registerCommand('rossi.buildCheckedRodinZip', (uri?: Uri) => controller.runCommand(() => controller.buildCheckedRodinZip(uri))),
        vscodeCommands.registerCommand('rossi.validateCurrentFile', (uri?: Uri) => controller.runCommand(() => controller.validateCurrentFile(uri))),
        vscodeCommands.registerCommand('rossi.validateWorkspace', () => controller.runCommand(() => controller.validateWorkspace())),
        vscodeCommands.registerCommand('rossi.convertCurrentFileToUnicode', (uri?: Uri) => controller.runCommand(() => controller.convertCurrentFileToUnicode(uri))),
        vscodeCommands.registerCommand('rossi.convertCurrentFileToAscii', (uri?: Uri) => controller.runCommand(() => controller.convertCurrentFileToAscii(uri))),
        vscodeCommands.registerCommand('rossi.checkToolchain', () => controller.runCommand(() => controller.checkToolchain())),
        vscodeCommands.registerCommand('rossi.newProject', () => controller.runCommand(() => controller.newProject()))
    );

    // On by default (`rossi.validate.onSave`): re-run the full project
    // validation when an .eventb file is saved. The setting is read at save time
    // so toggling it takes effect without a window reload. Saves are debounced so
    // a burst — Save All, format-on-save, or the extension's own programmatic
    // saves — coalesces into a single validate of the last-saved file's project
    // instead of spawning (and then cancelling) one CLI run per file.
    let saveDebounce: ReturnType<typeof setTimeout> | undefined;
    const onSave = workspace.onDidSaveTextDocument((document) => {
        if (!workspace.getConfiguration('rossi').get<boolean>('validate.onSave', true)) {
            return;
        }
        if (document.uri.scheme !== 'file' || !isEventBTextFile(document.uri.fsPath)) {
            return;
        }
        if (saveDebounce) {
            clearTimeout(saveDebounce);
        }
        saveDebounce = setTimeout(() => {
            saveDebounce = undefined;
            void controller.validateWorkspaceOnSave(document);
        }, ON_SAVE_DEBOUNCE_MS);
    });
    context.subscriptions.push(onSave, {
        dispose: () => {
            if (saveDebounce) {
                clearTimeout(saveDebounce);
            }
        },
    });
}

// The starter project keeps one component per .eventb file, matching the
// layout `rossi import` produces and the language server's one-component-per-
// document analysis (a file holding both would get a parse diagnostic at the
// second component).

/** Starter context written by the New Event-B Project command. */
function starterContext(name: string): string {
    return `CONTEXT ${name}_ctx
SETS
    S
CONSTANTS
    c
AXIOMS
    @axm1 c ∈ ℕ
END
`;
}

/** Starter machine written by the New Event-B Project command. */
function starterMachine(name: string): string {
    return `MACHINE ${name}
SEES
    ${name}_ctx
VARIABLES
    v
INVARIANTS
    @inv1 v ∈ ℕ
    @inv2 v ≤ c
EVENTS
    EVENT INITIALISATION
    BEGIN
        @act1 v := 0
    END

    EVENT step
    WHERE
        @grd1 v < c
    THEN
        @act1 v := v + 1
    END
END
`;
}

/** Getting-started README written into a new Event-B project. */
function starterReadme(name: string): string {
    return `# ${name}

An Event-B project edited with the Event-B (Rossi) extension.

## Getting started

1. Open \`${name}.eventb\` (the machine) and \`${name}_ctx.eventb\` (the
   context it sees) — one component per file, as in Rodin. Type \`context\`,
   \`machine\`, \`event\`, … and accept the snippet to scaffold a block.
2. Errors are reported live as you type by the Rossi language server, and on
   every save the whole project is validated for the type and dead-code checks
   the live server does not compute (turn off with \`rossi.validate.onSave\`).
3. Run **Rossi: Validate Current File** to validate on demand at any time.
4. Switch operator style with **Rossi: Convert Current File to Unicode** /
   **… to ASCII**.
5. Export with **Rossi: Export Current File to Rodin ZIP**, or
   **Rossi: Open in Rodin** to launch the Rodin IDE on this model.

Open the Command Palette (Ctrl/Cmd+Shift+P) and search for "Rossi" to see every command.
`;
}

const STARTER_GITIGNORE = `# Rossi / Event-B exported artifacts
*.zip

# OS files
.DS_Store
`;

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
    if (isEventBTextExtension(ext)) {
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

/**
 * Dot-named directories (`.rossi/rodin` Rodin workspaces, `.git`, Eclipse
 * `.metadata`) hold generated or foreign files, never sources. The CLI and
 * the language server both skip them, so scanning them here would hand
 * `rossi validate` generated sources neither of those would ever look at.
 */
function isGeneratedDirectory(name: string): boolean {
    return name.startsWith('.');
}

/**
 * Event-B text the CLI accepts from a directory scan. Deliberately wider than
 * the `.eventb` the language server indexes: `rossi validate` and `rossi fmt`
 * take a `.txt` too.
 */
function isEventBTextExtension(ext: string): boolean {
    return ext === '.eventb' || ext === '.txt';
}

async function scanDirectory(dir: string): Promise<{ hasEventB: boolean; hasRodinXml: boolean }> {
    const result = { hasEventB: false, hasRodinXml: false };
    const entries = await fs.readdir(dir, { withFileTypes: true });
    for (const entry of entries) {
        const entryPath = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            if (isGeneratedDirectory(entry.name)) {
                continue;
            }
            const child = await scanDirectory(entryPath);
            result.hasEventB ||= child.hasEventB;
            result.hasRodinXml ||= child.hasRodinXml;
        } else if (entry.isFile()) {
            const ext = path.extname(entry.name).toLowerCase();
            result.hasEventB ||= isEventBTextExtension(ext);
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
            if (isGeneratedDirectory(entry.name)) {
                continue;
            }
            files.push(...await collectEventBTextFiles(entryPath));
        } else if (entry.isFile()) {
            const ext = path.extname(entry.name).toLowerCase();
            if (isEventBTextExtension(ext)) {
                files.push(entryPath);
            }
        }
    }
    files.sort();
    return files;
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

async function pathExists(filePath: string): Promise<boolean> {
    try {
        await fs.stat(filePath);
        return true;
    } catch {
        return false;
    }
}


