/**
 * Standalone unit tests for the pure validation-region conversion. No VSCode
 * required.
 *
 *   npm run test:region
 *
 * Exits non-zero on the first failure so it can gate CI / pre-commit.
 */
import { regionToZeroIndexed, shownByServer } from './validationRegion';

let failures = 0;
function check(name: string, condition: boolean): void {
    if (condition) {
        console.log(`ok   ${name}`);
    } else {
        console.error(`FAIL ${name}`);
        failures += 1;
    }
}

// The reported case: `Union` on line 19, columns 5..10 (1-indexed, end
// exclusive) must map to line 18, characters [4, 9) (0-indexed, end exclusive
// — VS Code Range end is exclusive) — not the top of the file.
const union = regionToZeroIndexed({
    start_line: 19,
    start_column: 5,
    end_line: 19,
    end_column: 10,
});
check('start maps 1-indexed → 0-indexed', union.startLine === 18 && union.startChar === 4);
check('end maps 1-indexed → 0-indexed', union.endLine === 18 && union.endChar === 9);

// A 1:1 region (the smallest the CLI emits) clamps to the file origin.
const origin = regionToZeroIndexed({ start_line: 1, start_column: 1, end_line: 1, end_column: 1 });
check('1:1 region clamps to (0,0)', origin.startLine === 0 && origin.startChar === 0);

// No region → a one-character span at the file start (the legacy fallback).
const fallback = regionToZeroIndexed(undefined);
check(
    'missing region falls back to file start',
    fallback.startLine === 0 &&
        fallback.startChar === 0 &&
        fallback.endLine === 0 &&
        fallback.endChar === 1
);

// A finding the language server already shows is recognised on its start
// line; the message and column never take part.
{
    const at = (line: number) => ({ start_line: line, start_column: 3, end_line: line, end_column: 9 });
    const server = [
        { line: 4, code: 'EB022', isError: true },
        { line: 1, code: undefined, isError: true },
        { line: 7, code: 'EB029', isError: true },
    ];
    check('same code on the same line is shown', shownByServer('EB022', at(5), server));
    check('same code on another line is not', !shownByServer('EB022', at(6), server));
    check('another code on the same line is not', !shownByServer('EB006', at(5), server));
    check('EB004 matches an uncoded error on its line', shownByServer('EB004', at(2), server));
    check('EB004 does not match a coded error', !shownByServer('EB004', at(8), server));
    check('no region is never shown', !shownByServer('EB022', undefined, server));
    check('no rule is never shown', !shownByServer(undefined, at(5), server));
}

if (failures > 0) {
    console.error(`\n${failures} region test(s) failed.`);
    process.exit(1);
}
console.log('\nAll validation-region tests passed.');
