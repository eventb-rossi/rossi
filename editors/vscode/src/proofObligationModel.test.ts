/**
 * Standalone unit tests for the pure proof-obligation model. No VSCode
 * required.
 *
 *   npm run test:po
 *
 * Exits non-zero on the first failure so it can gate CI / pre-commit.
 */
import {
    Obligation,
    ProofStatus,
    groupObligations,
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

if (failures > 0) {
    console.error(`${failures} failure(s)`);
    process.exit(1);
}
console.log('all proof obligation model tests passed');
