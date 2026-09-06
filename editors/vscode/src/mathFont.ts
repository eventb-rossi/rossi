/**
 * Installing the bundled Rodin math font (Brave Sans Mono Roman) into the
 * user's own font directory.
 *
 * VS Code resolves `editor.fontFamily` only against fonts the operating system
 * knows about, and gives an extension no way to register one: the only
 * font-file contribution points, `contributes.icons` and
 * `contributes.productIconThemes`, produce workbench icons, not editor text. So
 * shipping `fonts/bravesansmono_roman.ttf` makes the file available but not
 * usable — something has to put it where the OS looks.
 *
 * That part an extension *can* do, because it is ordinary Node code running as
 * the user. Every platform has a per-user font location that needs no
 * administrator rights, so the command below is a plain copy plus whatever the
 * platform needs to notice it.
 *
 * The font is only needed for `rossi.format.privateUseGlyphs`; rossi's default
 * ASCII spelling of those four operators renders in any font.
 */
import { execFile } from 'child_process';
import * as fs from 'fs/promises';
import * as os from 'os';
import * as path from 'path';
import { promisify } from 'util';

const run = promisify(execFile);

/** File name of the bundled font, under the extension's `fonts/` directory. */
export const MATH_FONT_FILE = 'bravesansmono_roman.ttf';

/** Family name the font reports, as named in the `[eventb]` font stack. */
export const MATH_FONT_FAMILY = 'Brave Sans Mono';

/** Windows registers each per-user font under a display name of this shape. */
const WINDOWS_REGISTRY_NAME = 'Brave Sans Mono Roman (TrueType)';
const WINDOWS_FONT_REGISTRY_KEY = 'HKCU\\Software\\Microsoft\\Windows NT\\CurrentVersion\\Fonts';

/**
 * CSS generic families. One of these always resolves, so any family listed
 * after it is unreachable — which is why the math font has to go *before* the
 * generic rather than at the end of the stack.
 */
const GENERIC_FAMILIES = [
    'monospace',
    'ui-monospace',
    'sans-serif',
    'serif',
    'system-ui',
    'cursive',
    'fantasy',
];

/** A family name as written in a font stack, without quotes or padding. */
function bareFamily(family: string): string {
    return family.trim().replace(/^['"]|['"]$/g, '').toLowerCase();
}

/**
 * `current` with the math font inserted so it supplies only the code points the
 * user's own fonts lack — a font stack resolves per glyph, first match wins, so
 * the user's font must stay ahead of it. Returns `null` when the math font is
 * already listed and there is nothing to offer.
 */
export function fontFamilyWithMathFont(current: string): string | null {
    const families = current
        .split(',')
        .map((family) => family.trim())
        .filter((family) => family !== '');
    if (families.some((family) => bareFamily(family) === MATH_FONT_FAMILY.toLowerCase())) {
        return null;
    }
    const generic = families.findIndex((family) =>
        GENERIC_FAMILIES.includes(bareFamily(family))
    );
    const quoted = `'${MATH_FONT_FAMILY}'`;
    if (generic === -1) {
        families.push(quoted);
    } else {
        families.splice(generic, 0, quoted);
    }
    return families.join(', ');
}

export interface MathFontInstall {
    /** Per-user font directory, created if missing. */
    directory: string;
    /** Where the font file is copied to. */
    destination: string;
    /**
     * Command run after the copy so the platform picks the font up, or `null`
     * when dropping the file in the directory is enough (macOS). `required` is
     * true when the font stays invisible until the command succeeds: Windows
     * loads a per-user font from its registry value, never by scanning the
     * directory, so a failed `reg add` leaves the copy inert, whereas
     * fontconfig only needs its cache refreshed and finds the file anyway.
     */
    register: { command: string; args: string[]; required: boolean } | null;
}

/**
 * Where this platform keeps per-user fonts, and how it is told about a new one.
 * `null` for a platform with no such convention — the caller then reports that
 * the font must be installed by hand.
 *
 * Pure, so the per-platform layout is testable without touching a real font
 * directory; [`installMathFont`] supplies the ambient values.
 */
export function mathFontInstall(
    platform: NodeJS.Platform,
    home: string,
    env: Record<string, string | undefined>
): MathFontInstall | null {
    switch (platform) {
        case 'darwin': {
            // ~/Library/Fonts is scanned live; nothing to run afterwards.
            const directory = path.join(home, 'Library', 'Fonts');
            return {
                directory,
                destination: path.join(directory, MATH_FONT_FILE),
                register: null,
            };
        }
        case 'win32': {
            // Per-user font installs (no administrator rights) have been
            // supported since Windows 10 1803: the file lives under LOCALAPPDATA
            // and a registry value under HKCU names it by its full path.
            const localAppData = env.LOCALAPPDATA ?? path.join(home, 'AppData', 'Local');
            const directory = path.join(localAppData, 'Microsoft', 'Windows', 'Fonts');
            const destination = path.join(directory, MATH_FONT_FILE);
            return {
                directory,
                destination,
                register: {
                    command: 'reg',
                    args: [
                        'add', WINDOWS_FONT_REGISTRY_KEY,
                        '/v', WINDOWS_REGISTRY_NAME,
                        '/t', 'REG_SZ',
                        '/d', destination,
                        '/f',
                    ],
                    required: true,
                },
            };
        }
        case 'linux':
        case 'freebsd':
        case 'openbsd': {
            // fontconfig's per-user directory is `$XDG_DATA_HOME/fonts`, which
            // only defaults to ~/.local/share/fonts; a user who moved
            // XDG_DATA_HOME has nothing scanning the default path. The cache
            // must be rebuilt before an already-running fontconfig client sees
            // the new file.
            const dataHome = env.XDG_DATA_HOME || path.join(home, '.local', 'share');
            const directory = path.join(dataHome, 'fonts');
            return {
                directory,
                destination: path.join(directory, MATH_FONT_FILE),
                register: { command: 'fc-cache', args: ['-f', directory], required: false },
            };
        }
        default:
            return null;
    }
}

/**
 * Copy `source` into this platform's per-user font directory and register it.
 * Overwrites an existing copy, so re-running the command repairs a partial or
 * stale install rather than failing.
 *
 * Throws when the platform has no per-user font convention, when the copy
 * fails, or when a registration the platform requires fails. Refreshing
 * fontconfig's cache is best-effort — a font that is in place but whose cache
 * was not refreshed still shows up on the next scan — but Windows never scans
 * the directory, so reporting success there without the registry value would
 * claim an install that does not exist.
 */
export async function installMathFont(source: string): Promise<string> {
    const plan = mathFontInstall(os.platform(), os.homedir(), process.env);
    if (!plan) {
        throw new Error(
            `No per-user font directory is known for ${os.platform()}. ` +
            `Install ${source} with your desktop's font manager instead.`
        );
    }

    await fs.mkdir(plan.directory, { recursive: true });
    await fs.copyFile(source, plan.destination);

    if (plan.register) {
        try {
            await run(plan.register.command, plan.register.args);
        } catch (error) {
            if (plan.register.required) {
                throw new Error(
                    `Copied the font to ${plan.destination}, but registering it with ` +
                    `\`${plan.register.command}\` failed, so the system will not load it: ` +
                    `${error instanceof Error ? error.message : String(error)}`,
                    { cause: error }
                );
            }
            // The file is installed either way; the platform will pick it up on
            // its next scan, which a restart forces anyway.
        }
    }

    return plan.destination;
}
