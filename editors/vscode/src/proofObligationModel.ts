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

/** `$/rossi/proofStatus` parameters. */
export interface ProofStatusParams {
    uri: string;
    obligations: Obligation[];
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
    if (statuses.some((status) => status === 'broken')) {
        return 'broken';
    }
    return statuses.every(isClosed) ? 'closed' : 'open';
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
