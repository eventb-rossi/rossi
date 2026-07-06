import * as fs from 'fs/promises';
import * as path from 'path';

/**
 * Remove every cached toolchain version except the current one.
 *
 * The downloader keys its cache by extension version
 * (`…/globalStorage/rossi.event-b/bin/<version>/<triple>/`), so after an update
 * the previous version's binaries would otherwise linger forever. Because the
 * downloaded tools are pinned to the extension version, exactly one version dir
 * is ever wanted — `keepVersion` — and all siblings are stale.
 *
 * Best-effort garbage collection: it never throws. A version dir that cannot be
 * removed (e.g. a file is momentarily locked) is logged and left for the next
 * activation to retry, so a failure here can never break extension startup.
 * A missing `binRoot` (nothing has been downloaded yet) is a no-op.
 *
 * Only version directories directly under `binRoot` are considered; non-directory
 * entries are left untouched, and in-flight download staging dirs live one level
 * deeper (`bin/<version>/<triple>.tmp.<pid>`), so they are never at risk.
 */
export async function pruneStaleVersions(
    binRoot: string,
    keepVersion: string,
    log?: (message: string) => void
): Promise<void> {
    // A missing cache directory (nothing downloaded yet) or an unreadable one is
    // simply an empty prune — nothing to remove.
    const entries = await fs.readdir(binRoot, { withFileTypes: true }).catch(() => []);

    for (const entry of entries) {
        if (!entry.isDirectory() || entry.name === keepVersion) {
            continue;
        }
        const stale = path.join(binRoot, entry.name);
        try {
            await fs.rm(stale, { recursive: true, force: true });
            log?.(`Rossi: removed stale toolchain ${entry.name} from global storage.`);
        } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            log?.(`Rossi: could not remove stale toolchain ${stale} (${message}); will retry later.`);
        }
    }
}
