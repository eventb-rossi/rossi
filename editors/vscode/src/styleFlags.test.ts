/**
 * Standalone tests for the `rossi.format.*` → CLI flag mapping. No VSCode
 * required.
 *
 *   npm run test:styleflags
 *
 * Exits non-zero on the first failure so it can gate CI / pre-commit.
 */
import { formatStyleFlags, FormatSettings } from './styleFlags';

let failures = 0;
function check(name: string, condition: boolean): void {
    if (condition) {
        console.log(`ok   ${name}`);
    } else {
        console.error(`FAIL ${name}`);
        failures += 1;
    }
}

function eq(name: string, actual: string[], expected: string[]): void {
    check(name, JSON.stringify(actual) === JSON.stringify(expected));
    if (JSON.stringify(actual) !== JSON.stringify(expected)) {
        console.error(`  expected ${JSON.stringify(expected)}`);
        console.error(`  actual   ${JSON.stringify(actual)}`);
    }
}

const defaults: FormatSettings = {
    style: '',
    keywordCase: '',
    declLists: '',
    blankBetweenClauses: null,
    indentation: '',
    maxLineWidth: 120,
    privateUseGlyphs: false,
};

// Default settings follow the preset for everything but the line width,
// matching what the LSP formatter does with the same configuration.
eq('defaults pass only --max-width', formatStyleFlags(defaults), ['--max-width', '120']);

// The reported bug: style + declLists selected in the editor were dropped by
// the convert commands.
eq(
    'style and declLists map to their flags',
    formatStyleFlags({ ...defaults, style: 'rossi', declLists: 'one-per-line' }),
    ['--style', 'rossi', '--decl-lists', 'one-per-line', '--max-width', '120']
);

eq(
    'every override maps',
    formatStyleFlags({
        style: 'camille',
        keywordCase: 'upper',
        declLists: 'inline',
        blankBetweenClauses: false,
        indentation: '\t',
        maxLineWidth: 0,
        privateUseGlyphs: true,
    }),
    [
        '--style', 'camille',
        '--keyword-case', 'upper',
        '--decl-lists', 'inline',
        '--blank-between-clauses', 'false',
        '--indent', '\t',
        '--max-width', '0',
        '--private-use-glyphs',
    ]
);


// Tolerance mirrors the server: case-insensitive enums, unknown or mistyped
// values fall back to the preset instead of failing the CLI invocation.
eq(
    'enum values are case-insensitive',
    formatStyleFlags({ ...defaults, style: 'Rossi' }),
    ['--style', 'rossi', '--max-width', '120']
);
eq(
    'unknown enum values are dropped',
    formatStyleFlags({ ...defaults, style: 'compact', declLists: 'stacked' }),
    ['--max-width', '120']
);
eq(
    'mistyped values are dropped',
    formatStyleFlags({
        style: 42,
        keywordCase: true,
        declLists: undefined,
        blankBetweenClauses: 'false',
        indentation: 4,
        maxLineWidth: 'wide',
        privateUseGlyphs: 1,
    }),
    []
);
eq(
    'fractional or negative widths are dropped',
    formatStyleFlags({ ...defaults, maxLineWidth: -1 }),
    []
);
check(
    'true blankBetweenClauses maps',
    formatStyleFlags({ ...defaults, blankBetweenClauses: true }).join(' ').includes('--blank-between-clauses true')
);

if (failures > 0) {
    console.error(`\n${failures} style-flag test(s) failed.`);
    process.exit(1);
}
console.log('\nAll style-flag tests passed.');
