import * as os from 'os';

/**
 * Resolve a CLI working directory. `null` deliberately opts out of the
 * workspace directory for commands whose input is independent of it.
 */
export function resolveCommandCwd(
    requested: string | null | undefined,
    workspaceCwd: string | undefined
): string | undefined {
    return requested === null ? os.tmpdir() : requested ?? workspaceCwd;
}
