/**
 * The pure half of the proof obligation view: the wire shapes the server
 * sends and the grouping and counting the tree, the gutter and the status
 * bar all derive from them. No VS Code dependency, so it is unit-testable
 * with plain node, like `symbolMatcher`.
 */

/** `rossi prove`'s vocabulary, as `rossi/proofObligations` reports it. */
export type ProofStatus =
    | 'discharged'
    | 'reviewed'
    | 'pending'
    | 'unattempted'
    | 'broken'
    | 'unsupported'
    | 'error';

export interface LspPosition {
    line: number;
    character: number;
}

export interface LspRange {
    start: LspPosition;
    end: LspPosition;
}

/** One row of `rossi/proofObligations` and of `$/rossi/proofStatus`. */
export interface Obligation {
    name: string;
    component: string;
    description: string;
    range: LspRange;
    status: ProofStatus;
    accurate: boolean;
}

/**
 * A region the server folds obligations into: an event (INITIALISATION
 * included) or an invariants, theorems, variant or axioms clause. `header`
 * is the event name or the clause keyword, `range` the whole region.
 * Blocks never overlap; an obligation belongs to the block whose range
 * holds its start line.
 */
export interface Block {
    name: string;
    header: LspRange;
    range: LspRange;
}

/** `rossi/proofObligations` result. */
export interface ProofReport {
    obligations: Obligation[];
    blocks: Block[];
}

/** `$/rossi/proofStatus` parameters. */
export interface ProofStatusParams extends ProofReport {
    uri: string;
}

/** `rossi/proofState` result. */
export interface ProofState {
    name: string;
    component: string;
    description: string;
    identifiers: { name: string; type: string }[];
    hypotheses: string[];
    goal: string;
    text: string;
}

/** Closed means nothing is left to do: the server's own definition. */
function isClosed(status: ProofStatus): boolean {
    return status === 'discharged' || status === 'reviewed';
}

/** The marker a line gets when several obligations anchor on it. */
export type LineMark = 'closed' | 'open' | 'broken';

/**
 * The mark for a set of obligations sharing a line: broken wins (something
 * the user had was lost), then open, and closed only when every one is.
 */
export function markFor(statuses: ProofStatus[]): LineMark {
    return joinMarks(statuses.map(markOf));
}

function markOf(status: ProofStatus): LineMark {
    if (status === 'broken') {
        return 'broken';
    }
    return isClosed(status) ? 'closed' : 'open';
}

/**
 * The same precedence over marks that are already folded, so a block's mark
 * is the join of its lines' rather than a second fold of the statuses.
 */
function joinMarks(marks: Iterable<LineMark>): LineMark {
    let joined: LineMark = 'closed';
    for (const mark of marks) {
        if (mark === 'broken') {
            return 'broken';
        }
        if (mark === 'open') {
            joined = 'open';
        }
    }
    return joined;
}

/** The mark of every line that anchors an obligation. */
export function lineMarks(obligations: Obligation[]): Map<number, LineMark> {
    const byLine = new Map<number, ProofStatus[]>();
    for (const obligation of obligations) {
        const line = obligation.range.start.line;
        const statuses = byLine.get(line);
        if (statuses) {
            statuses.push(obligation.status);
        } else {
            byLine.set(line, [obligation.status]);
        }
    }
    const marks = new Map<number, LineMark>();
    for (const [line, statuses] of byLine) {
        marks.set(line, markFor(statuses));
    }
    return marks;
}

/** How the gutter shows proof status: `rossi.proofObligations.gutter`. */
export type GutterMode = 'off' | 'icons' | 'bars';

/**
 * One glyph-margin cell in `bars` mode: a status icon with a bar of the
 * same colour beside it, or the bar alone.
 */
export type GutterCell = `mark-${LineMark}` | `bar-${LineMark}`;

/** Anything the gutter can draw, each of it an `icons/po-<glyph>.svg`. */
export type GutterGlyph = LineMark | GutterCell;

const LINE_MARKS: readonly LineMark[] = ['closed', 'open', 'broken'];

export const GUTTER_GLYPHS: readonly GutterGlyph[] = [
    ...LINE_MARKS,
    ...LINE_MARKS.map((mark): GutterCell => `mark-${mark}`),
    ...LINE_MARKS.map((mark): GutterCell => `bar-${mark}`),
];

/**
 * The cell of every line drawn in `bars` mode. A block whose obligations
 * are all closed collapses to one check on its header line and a plain
 * bar down the rest; otherwise each line with obligations keeps its own
 * mark and the block's other lines carry a bar in the block's aggregate
 * colour (broken if any proof is broken, open otherwise). Obligations
 * outside every block keep a mark of their own; a block without
 * obligations draws nothing.
 */
export function gutterCells(report: ProofReport): Map<number, GutterCell> {
    const marks = lineMarks(report.obligations);
    const cells = new Map<number, GutterCell>();
    for (const block of report.blocks) {
        const first = block.range.start.line;
        const last = block.range.end.line;
        // Read off the marks already folded per line, rather than sweeping
        // the whole obligation list once per block.
        const inside: LineMark[] = [];
        for (let line = first; line <= last; line += 1) {
            const mark = marks.get(line);
            if (mark) {
                inside.push(mark);
            }
        }
        if (inside.length === 0) {
            continue;
        }
        const blockMark = joinMarks(inside);
        for (let line = first; line <= last; line += 1) {
            if (blockMark === 'closed') {
                cells.set(line, line === block.header.start.line ? 'mark-closed' : 'bar-closed');
            } else {
                const mark = marks.get(line);
                cells.set(line, mark ? `mark-${mark}` : `bar-${blockMark}`);
            }
        }
    }
    for (const [line, mark] of marks) {
        if (!cells.has(line)) {
            cells.set(line, `mark-${mark}`);
        }
    }
    return cells;
}

export interface Summary {
    closed: number;
    total: number;
}

export function summarize(obligations: Obligation[]): Summary {
    let closed = 0;
    for (const obligation of obligations) {
        if (isClosed(obligation.status)) {
            closed += 1;
        }
    }
    return { closed, total: obligations.length };
}

/** A component's obligations, split by the event they belong to. */
export interface ComponentGroup {
    component: string;
    groups: EventGroup[];
}

/**
 * The obligations under one event, or the component-level ones (invariant
 * and axiom well-definedness, theorems, variants) under `null`.
 */
export interface EventGroup {
    event: string | null;
    obligations: Obligation[];
}

/**
 * The event an obligation's split name belongs to, following the generator's
 * naming: `evt/label/SUFFIX` and `evt/SUFFIX` name an event, `label/SUFFIX`
 * a component-level element. A two-part name is ambiguous on its own
 * (`inv1/WD` versus `evt/MRG`), so the caller says which first segments are
 * events.
 */
function eventOf(parts: string[], events: Set<string>): string | null {
    if (parts.length >= 3 || (parts.length === 2 && events.has(parts[0]))) {
        return parts[0];
    }
    return null;
}

/**
 * Group obligations component by component, then event by event, keeping
 * the generator's order within each group. The events are those the
 * three-part names reveal; a two-part name whose first segment matches one
 * of them is an event-level obligation of that event.
 */
export function groupObligations(obligations: Obligation[]): ComponentGroup[] {
    // Split each name once: the event set and the grouping read the same parts.
    const split = obligations.map((obligation) => ({ obligation, parts: obligation.name.split('/') }));
    const events = new Set<string>();
    for (const { parts } of split) {
        if (parts.length >= 3) {
            events.add(parts[0]);
        }
    }

    const byComponent = new Map<string, Map<string | null, Obligation[]>>();
    for (const { obligation, parts } of split) {
        let groups = byComponent.get(obligation.component);
        if (!groups) {
            groups = new Map();
            byComponent.set(obligation.component, groups);
        }
        const event = eventOf(parts, events);
        let list = groups.get(event);
        if (!list) {
            list = [];
            groups.set(event, list);
        }
        list.push(obligation);
    }

    const result: ComponentGroup[] = [];
    for (const [component, groups] of byComponent) {
        const eventGroups: EventGroup[] = [];
        for (const [event, list] of groups) {
            eventGroups.push({ event, obligations: list });
        }
        // Component-level obligations first, then events in first-seen order.
        eventGroups.sort((a, b) => Number(a.event !== null) - Number(b.event !== null));
        result.push({ component, groups: eventGroups });
    }
    return result;
}
