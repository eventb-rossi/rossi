/**
 * Standalone tests for CLI working-directory selection. No VSCode required.
 *
 *   npm run test:cwd
 *
 * Exits non-zero on the first failure so it can gate CI / pre-commit.
 */
import { spawn } from 'child_process';
import * as fs from 'fs/promises';
import * as os from 'os';
import * as path from 'path';
import { resolveCommandCwd } from './commandCwd';

let failures = 0;
function check(name: string, condition: boolean): void {
    if (condition) {
        console.log(`ok   ${name}`);
    } else {
        console.error(`FAIL ${name}`);
        failures += 1;
    }
}

function spawnNode(cwd: string): Promise<string | undefined> {
    return new Promise((resolve) => {
        const child = spawn(process.execPath, ['--version'], { cwd, shell: false });
        child.on('error', (error: NodeJS.ErrnoException) => resolve(error.code));
        child.on('close', () => resolve(undefined));
    });
}

async function main(): Promise<void> {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), 'rossi-command-cwd-'));
    try {
        const missingWorkspace = path.join(root, 'removed');

        // Reproduce the reported failure: Node attributes ENOENT to the valid
        // executable even though only its working directory is absent.
        check('missing cwd reproduces spawn ENOENT', await spawnNode(missingWorkspace) === 'ENOENT');

        const independent = resolveCommandCwd(null, missingWorkspace);
        check('workspace-independent command uses the temp directory', independent === os.tmpdir());
        check(
            'workspace-independent cwd allows spawning',
            independent !== undefined && await spawnNode(independent) === undefined
        );
        check('explicit cwd wins', resolveCommandCwd(root, missingWorkspace) === root);
        check('default command uses the workspace cwd', resolveCommandCwd(undefined, missingWorkspace) === missingWorkspace);
    } finally {
        await fs.rm(root, { recursive: true, force: true });
    }

    if (failures > 0) {
        console.error(`\n${failures} command-cwd test(s) failed.`);
        process.exit(1);
    }
    console.log('\nAll command-cwd tests passed.');
}

main().catch((error) => {
    console.error(error);
    process.exit(1);
});
