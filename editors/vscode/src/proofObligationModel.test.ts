/**
 * Standalone unit tests for the pure proof-obligation model. No VSCode
 * required.
 *
 *   npm run test:po
 *
 * Exits non-zero on the first failure so it can gate CI / pre-commit.
 */
import {
    Block,
    GutterCell,
    Obligation,
    ProofStatus,
    groupObligations,
    gutterCells,
    lineMarks,
    markFor,
    summarize,
} from './proofObligationModel';

let failures = 0;
function check(name: string, condition: boolean): void {
    if (condition) {
        console.log(`ok   ${name}`);
    } else {
        console.error(`FAIL ${name}`);
        failures += 1;
    }
}

function obligation(name: string, status: ProofStatus, line = 0): Obligation {
    return {
        name,
        component: 'm',
        description: '',
        range: { start: { line, character: 0 }, end: { line, character: 1 } },
        status,
        accurate: true,
    };
}

// Grouping follows the generator's naming: three-part names belong to their
// event, two-part names to the component unless their first segment is a
// known event (`bump/MRG`).
const groups = groupObligations([
    obligation('inv1/WD', 'discharged'),
    obligation('INITIALISATION/inv1/INV', 'unattempted'),
    obligation('bump/inv1/INV', 'unattempted'),
    obligation('bump/MRG', 'pending'),
]);
check('one component', groups.length === 1 && groups[0].component === 'm');
check(
    'component-level group comes first',
    groups[0].groups[0].event === null && groups[0].groups[0].obligations[0].name === 'inv1/WD'
);
check(
    'events keep first-seen order',
    groups[0].groups[1].event === 'INITIALISATION' && groups[0].groups[2].event === 'bump'
);
check(
    'a two-part name under a known event joins that event',
    groups[0].groups[2].obligations.map((o) => o.name).join(',') === 'bump/inv1/INV,bump/MRG'
);

// The summary counts discharged and reviewed as closed, nothing else.
const summary = summarize([
    obligation('a/WD', 'discharged'),
    obligation('b/WD', 'reviewed'),
    obligation('c/WD', 'pending'),
    obligation('d/WD', 'broken'),
]);
check('closed counts discharged and reviewed', summary.closed === 2 && summary.total === 4);

// A line's mark: broken wins, then open, closed only when every one is.
check('all closed marks closed', markFor(['discharged', 'reviewed']) === 'closed');
check('one open marks open', markFor(['discharged', 'unattempted']) === 'open');
check('one broken marks broken', markFor(['discharged', 'broken', 'unattempted']) === 'broken');

// Per line, the mark folds every obligation anchored there.
const marks = lineMarks([
    obligation('e1/inv1/INV', 'discharged', 4),
    obligation('e2/inv1/INV', 'unattempted', 4),
    obligation('inv2/WD', 'discharged', 5),
]);
check(
    'lineMarks folds a line the way markFor does',
    marks.size === 2 && marks.get(4) === 'open' && marks.get(5) === 'closed'
);

// The bars-mode cells: an event on lines 7..11 with its name on line 7.
function block(name: string, header: number, first: number, last: number): Block {
    return {
        name,
        header: { start: { line: header, character: 10 }, end: { line: header, character: 14 } },
        range: { start: { line: first, character: 4 }, end: { line: last, character: 7 } },
    };
}
function cellsOf(obligations: Obligation[], blocks: Block[]): string {
    return [...gutterCells({ obligations, blocks })]
        .sort(([a], [b]) => a - b)
        .map(([line, cell]: [number, GutterCell]) => `${line}:${cell}`)
        .join(' ');
}
const event = block('bump', 7, 7, 11);
check(
    'a fully closed block is one check on its header and a bar below',
    cellsOf(
        [obligation('bump/grd1/WD', 'discharged', 8), obligation('bump/act1/FIS', 'reviewed', 9)],
        [event]
    ) === '7:mark-closed 8:bar-closed 9:bar-closed 10:bar-closed 11:bar-closed'
);
check(
    'a partial block keeps per-line marks and a gray bar elsewhere',
    cellsOf(
        [obligation('bump/grd1/WD', 'discharged', 8), obligation('bump/act1/FIS', 'unattempted', 9)],
        [event]
    ) === '7:bar-open 8:mark-closed 9:mark-open 10:bar-open 11:bar-open'
);
check(
    'a broken proof colours the block bar amber but not the closed line',
    cellsOf(
        [obligation('bump/grd1/WD', 'discharged', 8), obligation('bump/act1/FIS', 'broken', 9)],
        [event]
    ) === '7:bar-broken 8:mark-closed 9:mark-broken 10:bar-broken 11:bar-broken'
);
check(
    'an obligation outside every block keeps its mark; an empty block draws nothing',
    cellsOf([obligation('m/VAR', 'unattempted', 0)], [event]) === '0:mark-open'
);
check(
    'adjacent blocks keep their own colours and the gap stays empty',
    cellsOf(
        [obligation('inv1/WD', 'discharged', 4), obligation('bump/grd1/WD', 'pending', 8)],
        [block('INVARIANTS', 3, 3, 5), event]
    ) === '3:mark-closed 4:bar-closed 5:bar-closed 7:bar-open 8:mark-open 9:bar-open 10:bar-open 11:bar-open'
);

if (failures > 0) {
    console.error(`${failures} failure(s)`);
    process.exit(1);
}
console.log('all proof obligation model tests passed');
