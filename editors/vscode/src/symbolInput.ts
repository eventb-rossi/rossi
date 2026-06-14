/**
 * Editor-side Unicode input method for Event-B.
 *
 * Converts ASCII operator combos to Unicode as the user types — the layer that
 * the language server deliberately does NOT own (per-keystroke substitution
 * must be synchronous, local, and undo/cursor aware). All matching logic lives
 * in the pure `symbolMatcher` module; this file is only the VSCode glue.
 *
 * Two trigger modes (both on by default):
 *   - eager: symbolic combos (`=>`, `|->`, `<=>`) convert via maximal munch;
 *   - leader: a `\name` abbreviation (`\and`, `\to`, `\nat`) expands on a
 *     boundary character. The leader is `\` (pinned, matching Lean/Agda).
 *
 * v1 scope: single primary cursor. Multi-cursor / paste / programmatic edits
 * reset state and never corrupt the buffer. Conversion happens everywhere,
 * including comments (matches Lean); comment-awareness is a future refinement.
 */
import {
    Disposable,
    ExtensionContext,
    OutputChannel,
    Range,
    TextDocumentChangeEvent,
    TextEditor,
    TextEditorDecorationType,
    window,
    workspace,
} from 'vscode';
import { LanguageClient } from 'vscode-languageclient/node';
import {
    isNameChar,
    LEADER,
    leaderPrefixBefore,
    leaderTokenBefore,
    OperatorRow,
    stepEager,
    SymbolMatcher,
} from './symbolMatcher';

interface InputConfig {
    enabled: boolean;
    eager: boolean;
}

function readConfig(): InputConfig {
    const c = workspace.getConfiguration('rossi.input');
    return {
        enabled: c.get<boolean>('enabled', true),
        eager: c.get<boolean>('eager', true),
    };
}

export function registerSymbolInput(
    context: ExtensionContext,
    client: LanguageClient,
    ready: Promise<void>,
    output: OutputChannel
): void {
    const controller = new SymbolInputController(client, output);
    context.subscriptions.push(controller);
    void controller.init(ready);
}

class SymbolInputController implements Disposable {
    private matcher: SymbolMatcher | null = null;
    private config: InputConfig = readConfig();
    private readonly disposables: Disposable[] = [];
    private readonly decoration: TextEditorDecorationType;

    /** True while we apply our own edit, so we ignore the resulting change. */
    private applying = false;

    // The uncommitted eager run sitting in the document (single cursor): its
    // ASCII `text`, the offset `end` just after it, and its document `uri`.
    // Null when there is no run in progress.
    private pendingRun: { text: string; end: number; uri: string } | null = null;

    constructor(
        private readonly client: LanguageClient,
        private readonly output: OutputChannel
    ) {
        this.decoration = window.createTextEditorDecorationType({
            textDecoration: 'underline',
        });
    }

    async init(ready: Promise<void>): Promise<void> {
        this.disposables.push(
            this.decoration,
            workspace.onDidChangeConfiguration((e) => {
                if (e.affectsConfiguration('rossi.input')) {
                    this.config = readConfig();
                    if (!this.config.enabled) {
                        this.resetPending();
                    }
                    const ed = window.activeTextEditor;
                    if (ed) {
                        this.updateDecoration(ed);
                    }
                }
            }),
            workspace.onDidChangeTextDocument((e) => this.onChange(e)),
            // Cursor moves only refresh the decoration; the run is kept honest
            // by the contiguity check in `handleEager`, not by selection events
            // (every keystroke moves the cursor, so resetting here would break
            // multi-character operators).
            window.onDidChangeTextEditorSelection((e) =>
                this.updateDecoration(e.textEditor)
            ),
            window.onDidChangeActiveTextEditor((ed) => {
                this.resetPending();
                if (ed) {
                    this.updateDecoration(ed);
                }
            })
        );

        // The operator table is the single source of truth (Rust); fetch it
        // once the server is ready. Input stays inert if the request fails, but
        // we log why: a silent failure here disables all as-you-type conversion
        // (eager and leader alike) with no visible symptom, which is exactly how
        // a server-side `rossi/operatorTable` regression hid in plain sight.
        try {
            await ready;
            const rows = await this.client.sendRequest<OperatorRow[]>(
                'rossi/operatorTable'
            );
            this.matcher = new SymbolMatcher(rows);
            this.output.appendLine(
                `Symbol input ready: ${rows.length} operators loaded.`
            );
        } catch (err) {
            this.matcher = null;
            this.output.appendLine(
                `Symbol input disabled: failed to load the operator table ` +
                    `(rossi/operatorTable): ${err instanceof Error ? err.message : String(err)}`
            );
        }
    }

    private resetPending(): void {
        this.pendingRun = null;
    }

    /** Lookback window for `\name` scanning: the backslash plus the longest
     * leader name. Only used once the operator table (and matcher) has loaded. */
    private get leaderLookback(): number {
        return (this.matcher?.maxLeaderLen ?? 0) + 1;
    }

    private onChange(e: TextDocumentChangeEvent): void {
        if (this.applying || !this.config.enabled || !this.matcher) {
            return;
        }
        const editor = window.activeTextEditor;
        if (!editor || editor.document !== e.document) {
            return;
        }
        if (e.document.languageId !== 'eventb' || e.contentChanges.length === 0) {
            return;
        }

        const changes = e.contentChanges;
        // Only single-character user insertions drive input. Paste, deletion,
        // and multi-cursor edits reset state and are otherwise ignored (v1).
        if (
            changes.length !== 1 ||
            changes[0].rangeLength !== 0 ||
            changes[0].text.length !== 1
        ) {
            this.resetPending();
            this.updateDecoration(editor);
            return;
        }

        const ch = changes[0].text;
        const insertOffset = changes[0].rangeOffset;
        const cursorOffset = insertOffset + 1;
        const uri = e.document.uri.toString();

        // 1) Leader commit: a boundary char (not a name char, not the leader)
        //    typed right after a resolvable `\name`.
        if (!isNameChar(ch) && ch !== LEADER) {
            if (this.tryLeaderCommit(editor, insertOffset)) {
                this.resetPending();
                this.updateDecoration(editor);
                return;
            }
        }

        // 2) Eager substitution.
        if (this.config.eager) {
            this.handleEager(editor, uri, ch, insertOffset, cursorOffset);
        } else {
            this.resetPending();
        }
        this.updateDecoration(editor);
    }

    private handleEager(
        editor: TextEditor,
        uri: string,
        ch: string,
        insertOffset: number,
        cursorOffset: number
    ): void {
        // Keep the run only if this insertion is exactly contiguous with the
        // previous run end in the same document; otherwise start fresh.
        const run = this.pendingRun;
        const pending =
            run && run.uri === uri && run.end === insertOffset ? run.text : '';

        const action = stepEager(this.matcher!, pending, ch);
        switch (action.type) {
            case 'wait':
            case 'reset':
                this.pendingRun = { text: action.pending, end: cursorOffset, uri };
                break;
            case 'convertWithChar': {
                // `pending + ch` occupies [insertOffset - pending.length, cursor).
                const start = insertOffset - pending.length;
                this.resetPending();
                void this.replace(editor, start, cursorOffset, action.unicode);
                break;
            }
            case 'convertHeld': {
                // Replace the held run BEFORE the typed char; keep the char.
                const start = insertOffset - action.heldLen;
                this.resetPending();
                void this.replace(editor, start, insertOffset, action.unicode);
                break;
            }
        }
    }

    /** Resolve and replace a `\name` immediately before `boundaryOffset`. */
    private tryLeaderCommit(editor: TextEditor, boundaryOffset: number): boolean {
        const doc = editor.document;
        const from = Math.max(0, boundaryOffset - this.leaderLookback);
        const before = doc.getText(
            new Range(doc.positionAt(from), doc.positionAt(boundaryOffset))
        );
        const tok = leaderTokenBefore(before);
        if (!tok) {
            return false;
        }
        const glyph = this.matcher!.resolveLeader(tok.name);
        if (glyph === null) {
            return false;
        }
        const start = from + tok.start; // absolute offset of the backslash
        void this.replace(editor, start, boundaryOffset, glyph);
        return true;
    }

    /**
     * Replace [startOffset, endOffset) with `text` as a single, self-contained
     * undo step (so Ctrl+Z restores the ASCII), suppressing the resulting
     * change event via the `applying` guard.
     */
    private async replace(
        editor: TextEditor,
        startOffset: number,
        endOffset: number,
        text: string
    ): Promise<void> {
        const doc = editor.document;
        const range = new Range(doc.positionAt(startOffset), doc.positionAt(endOffset));
        this.applying = true;
        try {
            await editor.edit((b) => b.replace(range, text));
        } catch {
            // Ignore: the document may have changed underneath us.
        } finally {
            this.applying = false;
        }
    }

    private updateDecoration(editor: TextEditor): void {
        if (!this.config.enabled || !this.matcher || !editor.selection.isEmpty) {
            editor.setDecorations(this.decoration, []);
            return;
        }
        const doc = editor.document;
        const cursor = editor.selection.active;
        const offset = doc.offsetAt(cursor);
        const before = doc.getText(
            new Range(doc.positionAt(Math.max(0, offset - this.leaderLookback)), cursor)
        );
        const prefix = leaderPrefixBefore(before);
        const ranges = prefix
            ? [new Range(doc.positionAt(offset - prefix.length), cursor)]
            : [];
        editor.setDecorations(this.decoration, ranges);
    }

    dispose(): void {
        for (const d of this.disposables) {
            d.dispose();
        }
        this.disposables.length = 0;
    }
}
