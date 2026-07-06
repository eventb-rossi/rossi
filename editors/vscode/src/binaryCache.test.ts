/**
 * Standalone unit tests for the stale-toolchain pruner. No VSCode required.
 *
 *   npm run test:cache
 *
 * Exits non-zero on the first failure so it can gate CI / pre-commit.
 */
import * as fs from 'fs/promises';
import * as os from 'os';
import * as path from 'path';
import { pruneStaleVersions } from './binaryCache';

let failures = 0;
function check(name: string, condition: boolean): void {
    if (condition) {
        console.log(`ok   ${name}`);
    } else {
        console.error(`FAIL ${name}`);
        failures += 1;
    }
}

async function exists(target: string): Promise<boolean> {
    try {
        await fs.stat(target);
        return true;
    } catch {
        return false;
    }
}

async function main(): Promise<void> {
    // A missing bin root is a no-op, not an error.
    const missing = path.join(os.tmpdir(), 'rossi-bincache-does-not-exist-xyz');
    await fs.rm(missing, { recursive: true, force: true });
    let threw = false;
    try {
        await pruneStaleVersions(missing, '0.1.3');
    } catch {
        threw = true;
    }
    check('missing bin root does not throw', !threw);

    // A populated cache: three version dirs (each with a dummy binary) plus a
    // stray top-level file. Only the kept version and the stray file survive.
    const binRoot = await fs.mkdtemp(path.join(os.tmpdir(), 'rossi-bincache-'));
    try {
        for (const version of ['0.1.1', '0.1.2', '0.1.3']) {
            const triple = path.join(binRoot, version, 'x86_64-apple-darwin');
            await fs.mkdir(triple, { recursive: true });
            await fs.writeFile(path.join(triple, 'rossi'), 'dummy');
        }
        const stray = path.join(binRoot, 'not-a-version.txt');
        await fs.writeFile(stray, 'leave me');

        await pruneStaleVersions(binRoot, '0.1.3');

        check('kept version survives', await exists(path.join(binRoot, '0.1.3')));
        check('kept version binary intact', await exists(path.join(binRoot, '0.1.3', 'x86_64-apple-darwin', 'rossi')));
        check('older version 0.1.1 removed', !(await exists(path.join(binRoot, '0.1.1'))));
        check('older version 0.1.2 removed', !(await exists(path.join(binRoot, '0.1.2'))));
        check('stray non-directory entry untouched', await exists(stray));
    } finally {
        await fs.rm(binRoot, { recursive: true, force: true });
    }

    if (failures > 0) {
        console.error(`\n${failures} binary-cache test(s) failed.`);
        process.exit(1);
    }
    console.log('\nAll binary-cache tests passed.');
}

main().catch((error) => {
    console.error(error);
    process.exit(1);
});
