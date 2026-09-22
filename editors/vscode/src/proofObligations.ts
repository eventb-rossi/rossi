/**
 * The proof obligation surface: a tree view of the active file's
 * obligations, gutter marks and bars on the elements and blocks they are
 * about, a status bar count, and a read-only sequent document for the
 * obligation the user picks.
 *
 * Everything shown comes from the server: `rossi/proofObligations` lists a
 * file's obligations and the blocks (events, clauses) they sit in,
 * `$/rossi/proofStatus` pushes the report again whenever the server
 * recomputes it (on open and on save), and `rossi/proofState` returns one
 * obligation's sequent. The extension keeps no model of its own beyond the
 * last report per file.
 *
 * The sequent is a virtual `.eventb` document rather than a webview: it is
 * plain Event-B text, so a text document gets the language's grammar, the
 * math font and the operator input for free, in a fraction of the code a
 * panel would need.
 */
import {
    commands,
    Disposable,
    Event,
    EventEmitter,
    ExtensionContext,
    languages,
    OutputChannel,
    Position,
    ProviderResult,
    QuickPickItem,
    Range,
    Selection,
    StatusBarAlignment,
    StatusBarItem,
    TextDocumentContentProvider,
    TextEditor,
    TextEditorDecorationType,
    TextEditorRevealType,
    ThemeIcon,
    TreeDataProvider,
    TreeItem,
    TreeItemCollapsibleState,
    Uri,
    ViewColumn,
    window,
    workspace,
} from 'vscode';
import { LanguageClient } from 'vscode-languageclient/node';
import {
    ComponentGroup,
    EventGroup,
    GUTTER_GLYPHS,
    GutterGlyph,
    GutterMode,
    Obligation,
    ProofReport,
    ProofState,
    ProofStatus,
    ProofStatusParams,
    groupObligations,
    gutterCells,
    lineMarks,
    summarize,
} from './proofObligationModel';

const VIEW_ID = 'rossi.proofObligations';
const STATE_SCHEME = 'rossi-po';

/** A row of the tree: a component, an event group, or an obligation. */
type Node =
    | { kind: 'component'; uri: string; group: ComponentGroup }
    | { kind: 'event'; uri: string; group: EventGroup }
    | { kind: 'obligation'; uri: string; obligation: Obligation };

class ObligationTree implements TreeDataProvider<Node> {
    private readonly changed = new EventEmitter<Node | undefined>();
    readonly onDidChangeTreeData: Event<Node | undefined> = this.changed.event;

    /** The uri whose obligations the tree shows: the active editor's. */
    private activeUri: string | undefined;

    constructor(private readonly store: ObligationStore) {}

    /**
     * Always fires: the list the store holds for an unchanged uri moves too
     * (a save's push, a manual refresh), and a tree only re-queries the
     * provider when told to.
     */
    setActive(uri: string | undefined): void {
        this.activeUri = uri;
        this.changed.fire(undefined);
    }

    getTreeItem(node: Node): TreeItem {
        switch (node.kind) {
            case 'component': {
                let closed = 0;
                let total = 0;
                for (const group of node.group.groups) {
                    const summary = summarize(group.obligations);
                    closed += summary.closed;
                    total += summary.total;
                }
                const item = new TreeItem(
                    node.group.component,
                    TreeItemCollapsibleState.Expanded
                );
                item.description = `${closed}/${total}`;
                item.iconPath = new ThemeIcon('symbol-module');
                item.contextValue = 'component';
                return item;
            }
            case 'event': {
                const summary = summarize(node.group.obligations);
                // Component-level obligations (invariant and axiom
                // well-definedness, theorems, variants) have no event.
                const item = new TreeItem(
                    node.group.event ?? 'component level',
                    TreeItemCollapsibleState.Expanded
                );
                item.description = `${summary.closed}/${summary.total}`;
                item.iconPath = new ThemeIcon(node.group.event ? 'symbol-event' : 'symbol-module');
                item.contextValue = 'event';
                return item;
            }
            case 'obligation': {
                const { obligation } = node;
                const item = new TreeItem(obligation.name, TreeItemCollapsibleState.None);
                item.description = obligation.description;
                item.tooltip = `${obligation.description}\n${obligation.status}${
                    obligation.accurate ? '' : ' (inaccurate: some hypotheses were dropped)'
                }`;
                item.iconPath = statusIcon(obligation.status);
                item.contextValue = 'obligation';
                item.command = {
                    command: 'rossi.proofObligations.reveal',
                    title: 'Reveal',
                    arguments: [node.uri, obligation],
                };
                return item;
            }
        }
    }

    getChildren(node?: Node): ProviderResult<Node[]> {
        if (!node) {
            if (!this.activeUri) {
                return [];
            }
            const uri = this.activeUri;
            return groupObligations(this.store.get(uri).obligations).map((group) => ({
                kind: 'component',
                uri,
                group,
            }));
        }
        switch (node.kind) {
            case 'component':
                return node.group.groups.map((group) => ({
                    kind: 'event',
                    uri: node.uri,
                    group,
                }));
            case 'event':
                return node.group.obligations.map((obligation) => ({
                    kind: 'obligation',
                    uri: node.uri,
                    obligation,
                }));
            case 'obligation':
                return [];
        }
    }
}

function statusIcon(status: ProofStatus): ThemeIcon {
    switch (status) {
        case 'discharged':
            return new ThemeIcon('pass-filled');
        case 'reviewed':
            return new ThemeIcon('eye');
        case 'broken':
            return new ThemeIcon('warning');
        case 'unsupported':
        case 'error':
            return new ThemeIcon('question');
        case 'pending':
        case 'unattempted':
            return new ThemeIcon('circle-large-outline');
    }
}

/** The last report the server sent for each file. */
class ObligationStore {
    private readonly lists = new Map<string, ProofReport>();

    get(uri: string): ProofReport {
        return this.lists.get(uri) ?? { obligations: [], blocks: [] };
    }

    has(uri: string): boolean {
        return this.lists.has(uri);
    }

    set(uri: string, report: ProofReport): void {
        this.lists.set(uri, report);
    }

    delete(uri: string): void {
        this.lists.delete(uri);
    }
}

/** The sequent documents: `rossi-po:/<component>/<name>.eventb?<file uri>`. */
class ProofStateProvider implements TextDocumentContentProvider {
    private readonly changed = new EventEmitter<Uri>();
    readonly onDidChange = this.changed.event;

    constructor(
        private readonly client: LanguageClient,
        private readonly output: OutputChannel
    ) {}

    static uriFor(fileUri: string, obligation: Obligation): Uri {
        return Uri.from({
            scheme: STATE_SCHEME,
            path: `/${obligation.component}/${obligation.name}.eventb`,
            query: fileUri,
        });
    }

    invalidate(fileUri: string): void {
        for (const document of workspace.textDocuments) {
            if (document.uri.scheme === STATE_SCHEME && document.uri.query === fileUri) {
                this.changed.fire(document.uri);
            }
        }
    }

    async provideTextDocumentContent(uri: Uri): Promise<string> {
        // `/<component>/<name>.eventb`: the name may itself contain slashes.
        const path = uri.path.replace(/^\//, '').replace(/\.eventb$/, '');
        const slash = path.indexOf('/');
        const name = slash >= 0 ? path.slice(slash + 1) : path;
        try {
            const state = await this.client.sendRequest<ProofState | null>('rossi/proofState', {
                textDocument: { uri: uri.query },
                name,
            });
            if (!state) {
                return `// ${name}: no such obligation in the current model\n`;
            }
            return state.text;
        } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            this.output.appendLine(`rossi/proofState failed for ${name}: ${message}`);
            return `// ${name}: the sequent could not be computed (${message})\n`;
        }
    }
}

/**
 * Gutter decorations of the active editor: one icon per line with
 * obligations in `icons` mode, or the icon-and-bar cells of `bars` mode.
 * The bar cells are drawn with `cover` so the bar always fills the cell's
 * height and consecutive lines join into one line, the way Lean's progress
 * bar and Dafny's verification gutter are drawn.
 */
class GutterMarks implements Disposable {
    private readonly types: Record<GutterGlyph, TextEditorDecorationType>;

    constructor(context: ExtensionContext) {
        const file = (theme: 'light' | 'dark', glyph: GutterGlyph) =>
            Uri.joinPath(context.extensionUri, 'icons', 'gutter', theme, `${glyph}.svg`);
        const entries = GUTTER_GLYPHS.map((glyph): [GutterGlyph, TextEditorDecorationType] => [
            glyph,
            window.createTextEditorDecorationType({
                // A glyph is drawn at fixed colours, so each theme gets its
                // own: the muted tones that read on the dark chrome turn
                // muddy on white, and a gutter icon cannot take a theme
                // colour the way a ThemeIcon does.
                light: { gutterIconPath: file('light', glyph) },
                dark: { gutterIconPath: file('dark', glyph) },
                // Stretched to fill the cell, so the bars of consecutive
                // lines join into one rail.
                gutterIconSize: 'cover',
            }),
        ]);
        this.types = Object.fromEntries(entries) as Record<GutterGlyph, TextEditorDecorationType>;
    }

    apply(editor: TextEditor, report: ProofReport, mode: GutterMode): void {
        const ranges = new Map<GutterGlyph, Range[]>();
        const draw = (glyph: GutterGlyph, line: number) => {
            const drawn = ranges.get(glyph);
            const range = new Range(line, 0, line, 0);
            if (drawn) {
                drawn.push(range);
            } else {
                ranges.set(glyph, [range]);
            }
        };
        if (mode === 'icons') {
            for (const [line, mark] of lineMarks(report.obligations)) {
                draw(mark, line);
            }
        } else if (mode === 'bars') {
            for (const [line, cell] of gutterCells(report)) {
                draw(cell, line);
            }
        }
        // Every glyph is set, so switching modes clears the ones it drops.
        for (const glyph of GUTTER_GLYPHS) {
            editor.setDecorations(this.types[glyph], ranges.get(glyph) ?? []);
        }
    }

    clear(editor: TextEditor): void {
        for (const type of Object.values(this.types)) {
            editor.setDecorations(type, []);
        }
    }

    dispose(): void {
        for (const type of Object.values(this.types)) {
            type.dispose();
        }
    }
}

export function registerProofObligations(
    context: ExtensionContext,
    client: LanguageClient,
    ready: Promise<void>,
    output: OutputChannel
): void {
    const store = new ObligationStore();
    const tree = new ObligationTree(store);
    const marks = new GutterMarks(context);
    const stateProvider = new ProofStateProvider(client, output);
    const statusBar: StatusBarItem = window.createStatusBarItem(StatusBarAlignment.Left, 50);
    statusBar.command = `${VIEW_ID}.focus`;

    const enabled = () =>
        workspace.getConfiguration('rossi').get<boolean>('proofObligations.enabled', true);
    const gutter = () =>
        workspace.getConfiguration('rossi').get<GutterMode>('proofObligations.gutter', 'bars');

    const isEventB = (editor: TextEditor | undefined): editor is TextEditor =>
        !!editor && editor.document.languageId === 'eventb' && editor.document.uri.scheme === 'file';

    const render = (editor: TextEditor | undefined) => {
        if (!isEventB(editor) || !enabled()) {
            tree.setActive(undefined);
            statusBar.hide();
            if (editor) {
                marks.clear(editor);
            }
            return;
        }
        const uri = editor.document.uri.toString();
        const report = store.get(uri);
        tree.setActive(uri);
        marks.apply(editor, report, gutter());
        const summary = summarize(report.obligations);
        statusBar.text = `$(law) ${summary.closed}/${summary.total} POs`;
        statusBar.tooltip = 'Proof obligations discharged in this file (click to open the view)';
        statusBar.show();
    };

    // Ask for a file's list when nothing has been pushed for it yet: an
    // editor that was open before the server started, or a request racing
    // the server's own open-time refresh.
    const fetch = async (editor: TextEditor | undefined, force = false) => {
        if (!isEventB(editor) || !enabled()) {
            return;
        }
        const uri = editor.document.uri.toString();
        if (!force && store.has(uri)) {
            return;
        }
        try {
            await ready;
            const report = await client.sendRequest<ProofReport>('rossi/proofObligations', {
                textDocument: { uri },
            });
            store.set(uri, report);
            if (window.activeTextEditor?.document.uri.toString() === uri) {
                render(window.activeTextEditor);
            }
        } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            output.appendLine(`rossi/proofObligations failed: ${message}`);
        }
    };

    context.subscriptions.push(
        window.registerTreeDataProvider(VIEW_ID, tree),
        workspace.registerTextDocumentContentProvider(STATE_SCHEME, stateProvider),
        marks,
        statusBar,
        window.onDidChangeActiveTextEditor((editor) => {
            render(editor);
            void fetch(editor);
        }),
        workspace.onDidCloseTextDocument((document) => {
            store.delete(document.uri.toString());
        }),
        workspace.onDidChangeConfiguration((event) => {
            if (event.affectsConfiguration('rossi.proofObligations')) {
                render(window.activeTextEditor);
                void fetch(window.activeTextEditor, true);
            }
        }),
        commands.registerCommand(
            'rossi.proofObligations.reveal',
            async (uri: string, obligation: Obligation) => {
                const document = await workspace.openTextDocument(Uri.parse(uri));
                const editor = await window.showTextDocument(document, {
                    viewColumn: ViewColumn.One,
                    preserveFocus: false,
                });
                const { start, end } = obligation.range;
                const range = new Range(
                    new Position(start.line, start.character),
                    new Position(end.line, end.character)
                );
                editor.selection = new Selection(range.start, range.start);
                editor.revealRange(range, TextEditorRevealType.InCenterIfOutsideViewport);
                await showState(uri, obligation);
            }
        ),
        commands.registerCommand('rossi.proofObligations.showState', async () => {
            const editor = window.activeTextEditor;
            if (!isEventB(editor)) {
                void window.showInformationMessage('Open an Event-B file to pick a proof obligation.');
                return;
            }
            const uri = editor.document.uri.toString();
            await fetch(editor);
            const { obligations } = store.get(uri);
            if (obligations.length === 0) {
                void window.showInformationMessage('This file has no proof obligations.');
                return;
            }
            // Obligations on the cursor's line first, then the rest.
            const line = editor.selection.active.line;
            const items: (QuickPickItem & { obligation: Obligation })[] = [...obligations]
                .sort((a, b) => Number(b.range.start.line === line) - Number(a.range.start.line === line))
                .map((obligation) => ({
                    label: obligation.name,
                    description: obligation.status,
                    detail: obligation.description,
                    obligation,
                }));
            const picked = await window.showQuickPick(items, {
                placeHolder: 'Show the sequent of a proof obligation',
            });
            if (picked) {
                await showState(uri, picked.obligation);
            }
        }),
        commands.registerCommand('rossi.proofObligations.refresh', async () => {
            await fetch(window.activeTextEditor, true);
            stateProvider.invalidate(window.activeTextEditor?.document.uri.toString() ?? '');
        })
    );

    async function showState(fileUri: string, obligation: Obligation): Promise<void> {
        const uri = ProofStateProvider.uriFor(fileUri, obligation);
        const document = await workspace.openTextDocument(uri);
        await languages.setTextDocumentLanguage(document, 'eventb');
        await window.showTextDocument(document, {
            viewColumn: ViewColumn.Beside,
            preserveFocus: true,
            preview: true,
        });
    }

    // Pushed lists replace the file's, repaint if it is the active file, and
    // refresh any sequent document open for it: a save may have changed what
    // an obligation asks.
    void ready.then(() => {
        context.subscriptions.push(
            client.onNotification('$/rossi/proofStatus', (params: ProofStatusParams) => {
                store.set(params.uri, params);
                if (window.activeTextEditor?.document.uri.toString() === params.uri) {
                    render(window.activeTextEditor);
                }
                stateProvider.invalidate(params.uri);
            })
        );
        render(window.activeTextEditor);
        void fetch(window.activeTextEditor);
    });
}
