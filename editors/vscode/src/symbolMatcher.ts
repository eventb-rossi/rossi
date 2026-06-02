/**
 * Pure ASCII -> Unicode matching logic for the Event-B symbol input method.
 *
 * This module has NO `vscode` dependency so it can be unit-tested with plain
 * Node (see `symbolMatcher.test.ts`). All editor glue lives in `symbolInput.ts`.
 *
 * The operator data comes from the language server via the `rossi/operatorTable`
 * custom request, so the ASCII<->Unicode mapping is never duplicated here.
 *
 * Two trigger modes are supported:
 *
 *  - Eager (symbolic combos like `=>`, `|->`, `<=>`): substituted as soon as the
 *    typed run can no longer extend to a longer operator (maximal munch with a
 *    one-character lookahead). Alphabetic ops (`NAT`, `or`, `dom`) are excluded
 *    because converting them eagerly would block typing ordinary words.
 *
 *  - Leader (`\name`): an abbreviation that expands to Unicode on a boundary
 *    character. Covers named/alphabetic ops and anything reserved out of eager.
 *
 * The leader character `\` is reserved: it can never start an eager run. Which
 * operators are eager-eligible is decided by the server (the `eager` flag on
 * each row) so the policy lives with the operator data, not here.
 */

/** One operator spelling as returned by the `rossi/operatorTable` request. */
export interface OperatorRow {
    ascii: string;
    unicode: string;
    description: string;
    aliases: string[];
    /** No word characters (alphabetic ops are leader-only). */
    symbolic: boolean;
    /** Eligible for eager as-you-type substitution (decided server-side). */
    eager: boolean;
}

/** The leader character that begins a `\name` abbreviation. */
export const LEADER = '\\';

/** A character that can appear inside a leader name (`\forall`, `\nat1`). */
export function isNameChar(ch: string): boolean {
    return /^[A-Za-z0-9]$/.test(ch);
}

export class SymbolMatcher {
    /** Eager op ASCII -> Unicode, for O(1) exact-match lookup. */
    private readonly exactByAscii = new Map<string, string>();
    /** Every strict prefix of an eager op ASCII, for O(1) extend checks. */
    private readonly prefixes = new Set<string>();
    /** Leader name -> Unicode (aliases plus the alphabetic ASCII word forms). */
    private readonly leader = new Map<string, string>();
    /** Longest leader-name length; sizes the editor's `\name` lookback window. */
    readonly maxLeaderLen: number;

    constructor(rows: OperatorRow[]) {
        for (const r of rows) {
            if (r.eager) {
                this.exactByAscii.set(r.ascii, r.unicode);
                for (let i = 1; i < r.ascii.length; i++) {
                    this.prefixes.add(r.ascii.slice(0, i));
                }
            }
            for (const alias of r.aliases) {
                this.leader.set(alias, r.unicode);
            }
            // Alphabetic ASCII (NAT, or, POW, UNION, ...) doubles as a leader
            // name so `\NAT` / `\or` work alongside the curated aliases.
            if (!r.symbolic) {
                this.leader.set(r.ascii, r.unicode);
            }
        }
        this.maxLeaderLen = Math.max(
            0,
            ...Array.from(this.leader.keys(), (k) => k.length)
        );
    }

    /** True if some eager op is strictly longer than `s` and starts with `s`. */
    canExtend(s: string): boolean {
        return this.prefixes.has(s);
    }

    /** The Unicode glyph if `s` is exactly an eager op, else `null`. */
    exact(s: string): string | null {
        return this.exactByAscii.get(s) ?? null;
    }

    /** True if `s` could begin or be an eager op (worth holding as a run). */
    isEagerStart(s: string): boolean {
        return this.prefixes.has(s) || this.exactByAscii.has(s);
    }

    /** The Unicode glyph for a leader name (`forall`, `nat`, `to`), else `null`. */
    resolveLeader(name: string): string | null {
        return this.leader.get(name) ?? null;
    }
}

/**
 * The decision for one typed character in the eager state machine, expressed in
 * terms of document edits relative to the cursor. `pending` is the uncommitted
 * ASCII run currently sitting in the document immediately before the cursor.
 *
 *  - `wait`: no edit; the run grows to `pending`.
 *  - `convertWithChar`: the typed char completed an op that cannot extend;
 *    replace `pending + char` (which includes the just-typed char) with `unicode`.
 *  - `convertHeld`: the typed char broke a held complete op; replace the
 *    `heldLen` characters BEFORE the typed char with `unicode`, keeping the char.
 *  - `reset`: no edit; start a fresh run equal to `pending` ('' or the char).
 */
export type EagerAction =
    | { type: 'wait'; pending: string }
    | { type: 'reset'; pending: string }
    | { type: 'convertWithChar'; unicode: string }
    | { type: 'convertHeld'; unicode: string; heldLen: number };

/**
 * Advance the eager state machine by one typed character `ch`, given the
 * current uncommitted `pending` run. Pure: returns the action to apply.
 */
export function stepEager(
    matcher: SymbolMatcher,
    pending: string,
    ch: string
): EagerAction {
    // The leader char is reserved; it never participates in an eager run.
    if (ch === LEADER) {
        return { type: 'reset', pending: '' };
    }

    const candidate = pending + ch;

    if (matcher.canExtend(candidate)) {
        return { type: 'wait', pending: candidate };
    }

    const exactCandidate = matcher.exact(candidate);
    if (exactCandidate !== null) {
        return { type: 'convertWithChar', unicode: exactCandidate };
    }

    // `candidate` cannot extend and is not itself an op. If the run we were
    // holding (without `ch`) was a complete op, `ch` is its boundary: convert.
    const heldGlyph = matcher.exact(pending);
    if (heldGlyph !== null) {
        return { type: 'convertHeld', unicode: heldGlyph, heldLen: pending.length };
    }

    // Otherwise restart, seeding the new run with `ch` if it could begin an op.
    return { type: 'reset', pending: matcher.isEagerStart(ch) ? ch : '' };
}

/**
 * Simulate left-to-right typing of `input` through the eager state machine and
 * return the resulting text. Mirrors the editor glue exactly (including that a
 * `convertHeld` boundary clears the run rather than re-seeding), so it is the
 * reference oracle for unit tests. Leader expansion is NOT simulated here.
 */
export function simulateEagerTyping(matcher: SymbolMatcher, input: string): string {
    let out = '';
    let pending = '';
    for (const ch of input) {
        const action = stepEager(matcher, pending, ch);
        switch (action.type) {
            case 'wait':
            case 'reset':
                // Both keep the typed char and adopt the new run.
                out += ch;
                pending = action.pending;
                break;
            case 'convertWithChar':
                // `pending` is already in `out`; drop it and the (unwritten) char,
                // appending the glyph that subsumes both.
                out = out.slice(0, out.length - pending.length) + action.unicode;
                pending = '';
                break;
            case 'convertHeld':
                // Drop the held run from `out`, append the glyph, then the char.
                out = out.slice(0, out.length - action.heldLen) + action.unicode + ch;
                pending = '';
                break;
        }
    }
    return out;
}

/**
 * Find a `\name` leader token ending exactly at `text.length` (i.e. immediately
 * before the cursor). Returns the name and the backslash offset, or `null`.
 * Used both for committing on a boundary and for decorating the in-progress
 * abbreviation.
 */
export function leaderTokenBefore(
    text: string
): { name: string; start: number } | null {
    const m = /\\([A-Za-z][A-Za-z0-9]*)$/.exec(text);
    if (!m) {
        return null;
    }
    return { name: m[1], start: m.index };
}

/**
 * Find the in-progress leader prefix ending at the cursor: a backslash plus
 * zero or more name characters (so a lone `\` matches too). Used to decorate
 * the abbreviation as it is typed. Returns its offset and length, or `null`.
 */
export function leaderPrefixBefore(
    text: string
): { start: number; length: number } | null {
    const m = /\\[A-Za-z0-9]*$/.exec(text);
    return m ? { start: m.index, length: m[0].length } : null;
}
