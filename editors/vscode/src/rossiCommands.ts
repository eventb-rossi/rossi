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
import * as os from 'os';
import * as path from 'path';
import { regionToZeroIndexed, ValidationRegion } from './validationRegion';

interface RossiRunResult {
    stdout: string;
    stderr: string;
    exitCode: number | null;
}

interface RossiRunOptions {
    title: string;
    cwd?: string;
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

interface RodinProjectLaunch {
    workspaceDir: string;
    projectDir: string;
    projectName: string;
    importerDir: string;
    importerBuildFile: string;
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

    async openInRodin(uri?: Uri): Promise<void> {
        const input = await this.getOpenInRodinInput(uri);
        if (!input) {
            window.showErrorMessage('Open or select a .eventb file or folder to open in Rodin.');
            return;
        }

        const kind = await classifyInput(input);
        if (kind !== 'eventbFile' && kind !== 'eventbDirectory') {
            window.showErrorMessage('Open in Rodin supports .eventb files and folders containing .eventb files.');
            return;
        }

        await this.saveOpenEventBDocumentsUnder(input);

        try {
            const project = await this.prepareRodinProject(input);
            await this.launchRodin(project);
            window.showInformationMessage(`Opened ${project.projectName} in Rodin.`);
        } catch (error) {
            this.showCommandError(error);
        }
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
            // edits convert without forcing a save to disk.
            const buffer = await this.readEventBBuffer(input);
            const result = await this.runRossi(
                ['fmt', '-', ascii ? '--ascii' : '--unicode'],
                {
                    title: ascii ? 'Converting to ASCII' : 'Converting to Unicode',
                    cwd: path.dirname(input),
                    stdin: buffer,
                }
            );
            await this.replaceDocumentText(input, result.stdout);
            window.showInformationMessage(`Converted ${path.basename(input)} to ${ascii ? 'ASCII' : 'Unicode'}.`);
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

    private async prepareRodinProject(input: string): Promise<RodinProjectLaunch> {
        const projectName = rodinProjectName(input);
        const workspaceDir = await fs.mkdtemp(path.join(os.tmpdir(), 'rossi-rodin-workspace-'));
        const projectDir = path.join(workspaceDir, projectName);

        // `rossi export` to a directory path writes a complete Rodin project
        // (a `.project` descriptor plus each component's XML) that the importer
        // task below registers into the temporary Rodin workspace.
        await this.runRossi(
            ['export', input, '-o', projectDir],
            { title: 'Preparing Rodin project', cwd: path.dirname(input) }
        );

        const importerDir = await fs.mkdtemp(path.join(os.tmpdir(), 'rossi-rodin-importer-'));
        const importerBuildFile = path.join(importerDir, 'build.xml');
        await writeRodinImporterFiles(importerDir, importerBuildFile);

        return { workspaceDir, projectDir, projectName, importerDir, importerBuildFile };
    }

    private async launchRodin(project: RodinProjectLaunch): Promise<void> {
        const rodinPath = configuredRodinPath();

        await this.registerRodinProject(rodinPath, project);
        await this.suppressWelcomePage(project.workspaceDir);

        const launch = rodinLaunchCommand(rodinPath, project);
        this.output.appendLine(`> ${formatCommand(launch.command, launch.args)}`);

        await spawnDetached(launch.command, launch.args);
    }

    private async registerRodinProject(rodinPath: string, project: RodinProjectLaunch): Promise<void> {
        const command = await rodinAntRunnerCommand(rodinPath);
        try {
            await this.runExternal(command, [
                '-nosplash',
                '-application',
                'org.eclipse.ant.core.antRunner',
                '-data',
                project.workspaceDir,
                '-buildfile',
                project.importerBuildFile,
                `-DprojectDir=${project.projectDir}`,
            ], 'Registering Rodin project');
        } finally {
            await fs.rm(project.importerDir, { recursive: true, force: true });
        }
    }

    // A fresh Rodin workspace shows the Eclipse "Welcome" (intro) page on first
    // launch, hiding the imported project behind it. Pre-seed the workbench
    // preference so Rodin opens straight on the Event-B perspective. (Eclipse
    // flips this itself after showing the intro once, but we create a new temp
    // workspace every time, so we always hit the first-launch intro.) Best
    // effort: a cosmetic preference must never block opening Rodin.
    private async suppressWelcomePage(workspaceDir: string): Promise<void> {
        const settingsDir = path.join(
            workspaceDir,
            '.metadata',
            '.plugins',
            'org.eclipse.core.runtime',
            '.settings'
        );
        try {
            await fs.mkdir(settingsDir, { recursive: true });
            await fs.writeFile(
                path.join(settingsDir, 'org.eclipse.ui.prefs'),
                'eclipse.preferences.version=1\nshowIntro=false\n',
                'utf8'
            );
        } catch (error) {
            this.output.appendLine(`Could not suppress Rodin Welcome page: ${error}`);
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
        const cwd = options.cwd ?? workspace.workspaceFolders?.[0]?.uri.fsPath;
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

    private async runExternal(command: string, args: string[], title: string): Promise<RossiRunResult> {
        this.output.appendLine(`> ${formatCommand(command, args)}`);

        return window.withProgress(
            {
                location: ProgressLocation.Notification,
                title,
                cancellable: true,
            },
            (_progress, token) => this.spawnCommand(command, args, undefined, false, token)
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

    private async getOpenInRodinInput(uri?: Uri): Promise<string | undefined> {
        if (uri?.scheme === 'file') {
            return uri.fsPath;
        }

        const editorFile = await this.getEventBFile();
        if (editorFile) {
            return editorFile;
        }

        if (workspace.workspaceFolders && workspace.workspaceFolders.length > 0) {
            const workspaceFolder = await this.pickWorkspaceFolder();
            if (workspaceFolder) {
                return workspaceFolder;
            }
        }

        return this.pickInput('Open in Rodin', ['eventb'], true);
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
    cliPath: string,
    waitForLanguageServer?: () => Promise<void>
): void {
    const controller = new RossiCommandController(diagnostics, output, cliPath, waitForLanguageServer);
    context.subscriptions.push(
        vscodeCommands.registerCommand('rossi.importRodinProject', (uri?: Uri) => controller.runCommand(() => controller.importRodinProject(uri))),
        vscodeCommands.registerCommand('rossi.exportCurrentFileToRodinZip', (uri?: Uri) => controller.runCommand(() => controller.exportCurrentFileToRodinZip(uri))),
        vscodeCommands.registerCommand('rossi.exportWorkspaceToRodinZip', () => controller.runCommand(() => controller.exportWorkspaceToRodinZip())),
        vscodeCommands.registerCommand('rossi.buildCheckedRodinZip', (uri?: Uri) => controller.runCommand(() => controller.buildCheckedRodinZip(uri))),
        vscodeCommands.registerCommand('rossi.openInRodin', (uri?: Uri) => controller.runCommand(() => controller.openInRodin(uri))),
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

function configuredRodinPath(): string {
    const configured = workspace.getConfiguration('rossi').get<string>('rodin.path', '').trim();
    return configured || defaultRodinPath();
}

function defaultRodinPath(): string {
    switch (process.platform) {
        case 'darwin':
            return '/Applications/Rodin.app';
        case 'win32':
            return 'rodin.exe';
        default:
            return 'rodin';
    }
}

function rodinProjectName(input: string): string {
    const basename = path.basename(input, path.extname(input));
    const sanitized = basename.replace(/[^A-Za-z0-9_.-]/g, '_').replace(/^[.-]+/, '');
    return sanitized || 'rossi_project';
}

type MacRodinApp =
    | { kind: 'bundle'; appPath: string }
    | { kind: 'name'; appName: string };

// On macOS, interpret a configured Rodin path as either a `.app` bundle path or
// a bare application name (e.g. "Rodin"). Returns undefined on other platforms
// or for a plain executable path, so callers fall back to spawning it directly.
function macRodinApp(rodinPath: string): MacRodinApp | undefined {
    if (process.platform !== 'darwin') {
        return undefined;
    }
    if (rodinPath.toLowerCase().endsWith('.app')) {
        return { kind: 'bundle', appPath: rodinPath };
    }
    if (!hasPathSeparator(rodinPath) && /^[A-Z]/.test(rodinPath)) {
        return { kind: 'name', appName: rodinPath };
    }
    return undefined;
}

function rodinLaunchCommand(
    rodinPath: string,
    project: RodinProjectLaunch
): { command: string; args: string[] } {
    const args = ['-data', project.workspaceDir];
    const app = macRodinApp(rodinPath);

    if (app?.kind === 'bundle') {
        return { command: 'open', args: ['-n', app.appPath, '--args', ...args] };
    }
    if (app?.kind === 'name') {
        return { command: 'open', args: ['-n', '-a', app.appName, '--args', ...args] };
    }
    return { command: rodinPath, args };
}

function spawnDetached(command: string, args: string[]): Promise<void> {
    return new Promise((resolve, reject) => {
        const child = spawn(command, args, {
            detached: true,
            stdio: 'ignore',
            shell: false,
        });

        child.once('error', (error) => {
            reject(new Error(`Failed to start '${command}': ${error.message}`));
        });

        child.once('spawn', () => {
            child.unref();
            resolve();
        });
    });
}

function hasPathSeparator(value: string): boolean {
    return value.includes('/') || value.includes('\\');
}

async function rodinAntRunnerCommand(rodinPath: string): Promise<string> {
    const app = macRodinApp(rodinPath);

    if (app?.kind === 'bundle') {
        return macOsAppExecutable(app.appPath);
    }
    if (app?.kind === 'name') {
        return macOsAppExecutable(path.join('/Applications', `${app.appName}.app`));
    }
    return rodinPath;
}

async function macOsAppExecutable(appPath: string): Promise<string> {
    const executableDir = path.join(appPath, 'Contents', 'MacOS');
    const preferred = path.join(
        executableDir,
        path.basename(appPath, '.app').toLowerCase()
    );
    if (await pathExists(preferred)) {
        return preferred;
    }

    let entries: import('fs').Dirent[];
    try {
        entries = await fs.readdir(executableDir, { withFileTypes: true });
    } catch (error) {
        throw new Error(`Cannot find Rodin executable inside ${appPath}`, { cause: error });
    }

    const executable = entries.find((entry) => entry.isFile());
    if (!executable) {
        throw new Error(`Cannot find Rodin executable inside ${appPath}`);
    }
    return path.join(executableDir, executable.name);
}

async function writeRodinImporterFiles(importerDir: string, buildFile: string): Promise<void> {
    const classFile = path.join(
        importerDir,
        'org',
        'rossi',
        'vscode',
        'RodinProjectImportTask.class'
    );
    await fs.mkdir(path.dirname(classFile), { recursive: true });
    await fs.writeFile(classFile, Buffer.from(RODIN_PROJECT_IMPORT_TASK_CLASS_BASE64, 'base64'));
    await fs.writeFile(buildFile, rodinImporterBuildXml(importerDir), 'utf8');
}

function rodinImporterBuildXml(importerDir: string): string {
    return [
        '<project name="rossi-rodin-import" default="import">',
        `  <taskdef name="rossiImportProject" classname="org.rossi.vscode.RodinProjectImportTask" classpath="${escapeXmlAttribute(importerDir)}"/>`,
        '  <target name="import">',
        '    <rossiImportProject projectDir="${projectDir}"/>',
        '  </target>',
        '</project>',
        '',
    ].join('\n');
}

async function pathExists(filePath: string): Promise<boolean> {
    try {
        await fs.stat(filePath);
        return true;
    } catch {
        return false;
    }
}

function escapeXmlAttribute(value: string): string {
    return value
        .replace(/&/g, '&amp;')
        .replace(/"/g, '&quot;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;');
}

// Java 17 Ant task that calls Eclipse ResourcesPlugin to register the generated
// .project. The base64 below is the compiled class; the reviewable source and
// regeneration instructions live in `editors/vscode/rodin/`.
const RODIN_PROJECT_IMPORT_TASK_CLASS_BASE64 = [
    'yv66vgAAAD0AlAoAAgADBwAEDAAFAAYBABlvcmcvYXBhY2hlL3Rvb2xzL2FudC9UYXNrAQAGPGluaXQ+AQADKClWCQAIAAkH',
    'AAoMAAsADAEAJ29yZy9yb3NzaS92c2NvZGUvUm9kaW5Qcm9qZWN0SW1wb3J0VGFzawEACnByb2plY3REaXIBABJMamF2YS9s',
    'YW5nL1N0cmluZzsKAA4ADwcAEAwAEQASAQAQamF2YS9sYW5nL1N0cmluZwEAB2lzQmxhbmsBAAMoKVoHABQBACNvcmcvYXBh',
    'Y2hlL3Rvb2xzL2FudC9CdWlsZEV4Y2VwdGlvbggAFgEAFnByb2plY3REaXIgaXMgcmVxdWlyZWQKABMAGAwABQAZAQAVKExq',
    'YXZhL2xhbmcvU3RyaW5nOylWBwAbAQAMamF2YS9pby9GaWxlCgAaABgKABoAHgwAHwAgAQAQZ2V0Q2Fub25pY2FsRmlsZQEA',
    'ECgpTGphdmEvaW8vRmlsZTsIACIBAAgucHJvamVjdAoAGgAkDAAFACUBACMoTGphdmEvaW8vRmlsZTtMamF2YS9sYW5nL1N0',
    'cmluZzspVgoAGgAnDAAoABIBAAZpc0ZpbGUKAA4AKgwAKwAsAQAHdmFsdWVPZgEAJihMamF2YS9sYW5nL09iamVjdDspTGph',
    'dmEvbGFuZy9TdHJpbmc7EgAAAC4MAC8AMAEAF21ha2VDb25jYXRXaXRoQ29uc3RhbnRzAQAmKExqYXZhL2xhbmcvU3RyaW5n',
    'OylMamF2YS9sYW5nL1N0cmluZzsKADIAMwcANAwANQA2AQAqb3JnL2VjbGlwc2UvY29yZS9yZXNvdXJjZXMvUmVzb3VyY2Vz',
    'UGx1Z2luAQAMZ2V0V29ya3NwYWNlAQApKClMb3JnL2VjbGlwc2UvY29yZS9yZXNvdXJjZXMvSVdvcmtzcGFjZTsHADgBAB1v',
    'cmcvZWNsaXBzZS9jb3JlL3J1bnRpbWUvUGF0aAoAGgA6DAA7ADwBAA9nZXRBYnNvbHV0ZVBhdGgBABQoKUxqYXZhL2xhbmcv',
    'U3RyaW5nOwoANwAYCwA/AEAHAEEMAEIAQwEAJW9yZy9lY2xpcHNlL2NvcmUvcmVzb3VyY2VzL0lXb3Jrc3BhY2UBABZsb2Fk',
    'UHJvamVjdERlc2NyaXB0aW9uAQBSKExvcmcvZWNsaXBzZS9jb3JlL3J1bnRpbWUvSVBhdGg7KUxvcmcvZWNsaXBzZS9jb3Jl',
    'L3Jlc291cmNlcy9JUHJvamVjdERlc2NyaXB0aW9uOwsAPwBFDABGAEcBAAdnZXRSb290AQAtKClMb3JnL2VjbGlwc2UvY29y',
    'ZS9yZXNvdXJjZXMvSVdvcmtzcGFjZVJvb3Q7CwBJAEoHAEsMAEwAPAEALm9yZy9lY2xpcHNlL2NvcmUvcmVzb3VyY2VzL0lQ',
    'cm9qZWN0RGVzY3JpcHRpb24BAAdnZXROYW1lCwBOAE8HAFAMAFEAUgEAKW9yZy9lY2xpcHNlL2NvcmUvcmVzb3VyY2VzL0lX',
    'b3Jrc3BhY2VSb290AQAKZ2V0UHJvamVjdAEAOShMamF2YS9sYW5nL1N0cmluZzspTG9yZy9lY2xpcHNlL2NvcmUvcmVzb3Vy',
    'Y2VzL0lQcm9qZWN0OwcAVAEALG9yZy9lY2xpcHNlL2NvcmUvcnVudGltZS9OdWxsUHJvZ3Jlc3NNb25pdG9yCgBTAAMLAFcA',
    'WAcAWQwAWgASAQAjb3JnL2VjbGlwc2UvY29yZS9yZXNvdXJjZXMvSVByb2plY3QBAAZleGlzdHMLAFcAXAwAXQBeAQAGY3Jl',
    'YXRlAQBeKExvcmcvZWNsaXBzZS9jb3JlL3Jlc291cmNlcy9JUHJvamVjdERlc2NyaXB0aW9uO0xvcmcvZWNsaXBzZS9jb3Jl',
    'L3J1bnRpbWUvSVByb2dyZXNzTW9uaXRvcjspVgsAVwBgDABhABIBAAZpc09wZW4LAFcAYwwAZABlAQAEb3BlbgEALihMb3Jn',
    'L2VjbGlwc2UvY29yZS9ydW50aW1lL0lQcm9ncmVzc01vbml0b3I7KVYHAGcBACRvcmcvZWNsaXBzZS9jb3JlL3Jlc291cmNl',
    'cy9JUmVzb3VyY2ULAFcAaQwAagBrAQAMcmVmcmVzaExvY2FsAQAvKElMb3JnL2VjbGlwc2UvY29yZS9ydW50aW1lL0lQcm9n',
    'cmVzc01vbml0b3I7KVYLAD8AbQwAbgBvAQAEc2F2ZQEAUChaTG9yZy9lY2xpcHNlL2NvcmUvcnVudGltZS9JUHJvZ3Jlc3NN',
    'b25pdG9yOylMb3JnL2VjbGlwc2UvY29yZS9ydW50aW1lL0lTdGF0dXM7EgABAHEMAC8AcgEAOChMamF2YS9sYW5nL1N0cmlu',
    'ZztMamF2YS9sYW5nL1N0cmluZzspTGphdmEvbGFuZy9TdHJpbmc7CgAIAHQMAHUAGQEAA2xvZwcAdwEAE2phdmEvbGFuZy9F',
    'eGNlcHRpb24KABMAeQwABQB6AQAYKExqYXZhL2xhbmcvVGhyb3dhYmxlOylWAQAEQ29kZQEAD0xpbmVOdW1iZXJUYWJsZQEA',
    'DXNldFByb2plY3REaXIBAAdleGVjdXRlAQANU3RhY2tNYXBUYWJsZQEACkV4Y2VwdGlvbnMBAApTb3VyY2VGaWxlAQAbUm9k',
    'aW5Qcm9qZWN0SW1wb3J0VGFzay5qYXZhAQAQQm9vdHN0cmFwTWV0aG9kcwgAhQEAGE1pc3NpbmcgLnByb2plY3QgZmlsZTog',
    'AQgAhwEAH0ltcG9ydGVkIFJvZGluIHByb2plY3QgASBmcm9tIAEPBgCJCgCKAIsHAIwMAC8AjQEAJGphdmEvbGFuZy9pbnZv',
    'a2UvU3RyaW5nQ29uY2F0RmFjdG9yeQEAmChMamF2YS9sYW5nL2ludm9rZS9NZXRob2RIYW5kbGVzJExvb2t1cDtMamF2YS9s',
    'YW5nL1N0cmluZztMamF2YS9sYW5nL2ludm9rZS9NZXRob2RUeXBlO0xqYXZhL2xhbmcvU3RyaW5nO1tMamF2YS9sYW5nL09i',
    'amVjdDspTGphdmEvbGFuZy9pbnZva2UvQ2FsbFNpdGU7AQAMSW5uZXJDbGFzc2VzBwCQAQAlamF2YS9sYW5nL2ludm9rZS9N',
    'ZXRob2RIYW5kbGVzJExvb2t1cAcAkgEAHmphdmEvbGFuZy9pbnZva2UvTWV0aG9kSGFuZGxlcwEABkxvb2t1cAAxAAgAAgAA',
    'AAEAAgALAAwAAAADAAEABQAGAAEAewAAAB0AAQABAAAABSq3AAGxAAAAAQB8AAAABgABAAAADwABAH0AGQABAHsAAAAiAAIA',
    'AgAAAAYqK7UAB7EAAAABAHwAAAAKAAIAAAATAAUAFAABAH4ABgACAHsAAAGkAAQABwAAAOIqtAAHxgANKrQAB7YADZkADbsA',
    'E1kSFbcAF7+7ABpZKrQAB7cAHLYAHUy7ABpZKxIhtwAjTSy2ACaaABS7ABNZLLgAKboALQAAtwAXv7gAMU4tuwA3WSy2ADm3',
    'AD25AD4CADoELbkARAEAGQS5AEgBALkATQIAOgW7AFNZtwBVOgYZBbkAVgEAmgAOGQUZBBkGuQBbAwAZBbkAXwEAmgAMGQUZ',
    'BrkAYgIAGQUFGQa5AGgDAC0EGQa5AGwDAFcqGQS5AEgBACu4ACm6AHAAALYAc6cAEEwrv0y7ABNZK7cAeL+xAAIAGwDRANQA',
    'EwAbANEA1wB2AAIAfAAAAF4AFwAAABgAEQAZABsAHQAqAB4ANQAfADwAIABNACMAUQAkAGQAJQB4ACYAgQAoAIsAKQCWACsA',
    'oAAsAKkALgCzAC8AvQAwANEANQDUADEA1QAyANcAMwDYADQA4QA2AH8AAAA8AAgRCf0AMQcAGgcAGv8ASAAHBwAIBwAaBwAa',
    'BwA/BwBJBwBXBwBTAAAS/wAqAAEHAAgAAQcAE0IHAHYJAIAAAAAEAAEAEwADAIEAAAACAIIAgwAAAA4AAgCIAAEAhACIAAEA',
    'hgCOAAAACgABAI8AkQCTABk=',
].join('');
