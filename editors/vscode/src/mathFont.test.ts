/**
 * Standalone tests for the per-user font install layout. No VSCode required.
 *
 *   npm run test:mathfont
 *
 * Only the pure planning function is exercised: the platform's directory and
 * the command that registers the font are the parts that are easy to get wrong
 * and impossible to check from a single developer machine, and asserting them
 * here costs nothing. The copy itself is one `fs.copyFile`.
 *
 * Exits non-zero on the first failure so it can gate CI / pre-commit.
 */
import * as path from 'path';
import { fontFamilyWithMathFont, mathFontInstall, MATH_FONT_FILE } from './mathFont';

let failures = 0;
function check(name: string, condition: boolean): void {
    if (condition) {
        console.log(`ok   ${name}`);
    } else {
        console.error(`FAIL ${name}`);
        failures += 1;
    }
}

function eq(name: string, actual: unknown, expected: unknown): void {
    check(name, JSON.stringify(actual) === JSON.stringify(expected));
    if (JSON.stringify(actual) !== JSON.stringify(expected)) {
        console.error(`  expected ${JSON.stringify(expected)}`);
        console.error(`  actual   ${JSON.stringify(actual)}`);
    }
}

const HOME = path.join(path.sep + 'home', 'u');

// macOS scans ~/Library/Fonts live, so the copy is the whole install.
const mac = mathFontInstall('darwin', HOME, {});
eq('macOS uses ~/Library/Fonts', mac?.directory, path.join(HOME, 'Library', 'Fonts'));
eq('macOS destination is the font file', mac?.destination, path.join(HOME, 'Library', 'Fonts', MATH_FONT_FILE));
check('macOS needs no registration step', mac?.register === null);

// fontconfig caches aggressively; a stale cache hides a font that is in place.
const linux = mathFontInstall('linux', HOME, {});
eq('Linux uses ~/.local/share/fonts', linux?.directory, path.join(HOME, '.local', 'share', 'fonts'));
eq('Linux refreshes the fontconfig cache', linux?.register, {
    command: 'fc-cache',
    args: ['-f', path.join(HOME, '.local', 'share', 'fonts')],
    required: false,
});

// fontconfig scans $XDG_DATA_HOME/fonts, not the literal ~/.local/share/fonts,
// so a moved XDG_DATA_HOME must move the install with it.
const xdg = mathFontInstall('linux', HOME, { XDG_DATA_HOME: path.join(HOME, 'data') });
eq('Linux honours XDG_DATA_HOME', xdg?.directory, path.join(HOME, 'data', 'fonts'));

// Per-user installs on Windows need no administrator rights, but the file alone
// is not enough: the font is only visible once HKCU names it by full path.
const windows = mathFontInstall('win32', HOME, { LOCALAPPDATA: path.join(HOME, 'AppData', 'Local') });
const windowsDir = path.join(HOME, 'AppData', 'Local', 'Microsoft', 'Windows', 'Fonts');
eq('Windows uses the LOCALAPPDATA font directory', windows?.directory, windowsDir);
// Windows loads a per-user font from the registry, never by scanning the
// directory, so a failed registration is an install failure, not a hiccup.
eq('Windows registers the font under HKCU, and must', windows?.register, {
    command: 'reg',
    args: [
        'add', 'HKCU\\Software\\Microsoft\\Windows NT\\CurrentVersion\\Fonts',
        '/v', 'Brave Sans Mono Roman (TrueType)',
        '/t', 'REG_SZ',
        '/d', path.join(windowsDir, MATH_FONT_FILE),
        '/f',
    ],
    required: true,
});

// LOCALAPPDATA is normally set, but a stripped environment must not put the
// font at an "undefined" path.
const noLocalAppData = mathFontInstall('win32', HOME, {});
eq('Windows falls back to AppData\\Local under the home directory', noLocalAppData?.directory, windowsDir);

// A platform with no per-user font convention is reported rather than guessed.
check('an unknown platform has no install plan', mathFontInstall('aix', HOME, {}) === null);

// A font stack resolves per glyph, first match wins, so the user's own families
// must stay ahead of the math font or it would restyle text they can already
// read. A generic family always resolves, so anything after one is unreachable.
eq(
    'the math font goes before the generic family, after the real ones',
    fontFamilyWithMathFont("Menlo, Monaco, 'Courier New', monospace"),
    "Menlo, Monaco, 'Courier New', 'Brave Sans Mono', monospace"
);
eq(
    'a stack with no generic family gets the math font appended',
    fontFamilyWithMathFont('Fira Code'),
    "Fira Code, 'Brave Sans Mono'"
);
eq(
    'an empty stack yields the math font alone',
    fontFamilyWithMathFont(''),
    "'Brave Sans Mono'"
);
// Nothing to offer when it is already listed, however it was spelled.
check('an existing entry is detected', fontFamilyWithMathFont("Menlo, 'Brave Sans Mono'") === null);
check('detection ignores quoting', fontFamilyWithMathFont('Menlo, Brave Sans Mono') === null);
check('detection ignores case', fontFamilyWithMathFont('Menlo, "brave sans mono"') === null);

if (failures > 0) {
    console.error(`\n${failures} math-font test(s) failed.`);
    process.exit(1);
}
console.log('\nAll math-font tests passed.');
