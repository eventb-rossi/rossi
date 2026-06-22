// Verify a packaged .vsix: the file manifest must match expected-vsix-files.txt
// exactly, and the bundled entry point must load with all dependencies resolved.
//
// Usage:
//   node scripts/check-vsix.js [path-to.vsix] [--update]
//
// With no path, checks <name>-<version>.vsix next to package.json
// (build it first with `npm run package`). --update rewrites the expected
// manifest from the vsix instead of failing on a mismatch.
//
// The load check stubs only the 'vscode' module; any other bare (non-builtin)
// require means a dependency was not bundled into the vsix and would crash
// activate() at load time — the failure mode this guard exists to catch.
'use strict';

const { spawnSync } = require('child_process');
const fs = require('fs');
const Module = require('module');
const os = require('os');
const path = require('path');

const extensionDir = path.dirname(__dirname);
const manifestPath = path.join(__dirname, 'expected-vsix-files.txt');
const pkg = require(path.join(extensionDir, 'package.json'));

const args = process.argv.slice(2);
const update = args.includes('--update');
const vsixArg = args.find(a => a !== '--update');
const vsixPath = vsixArg
    ? path.resolve(vsixArg)
    : path.join(extensionDir, `${pkg.name}-${pkg.version}.vsix`);

if (!fs.existsSync(vsixPath)) {
    console.error(`vsix not found: ${vsixPath} (build it with \`npm run package\`)`);
    process.exit(2);
}

function unzip(argv) {
    const res = spawnSync('unzip', argv, { encoding: 'utf8' });
    if (res.status !== 0) {
        console.error(res.stderr || `unzip ${argv.join(' ')} failed`);
        process.exit(2);
    }
    return res.stdout;
}

// 1. File manifest must match the committed expected list exactly.
const actual = unzip(['-Z1', vsixPath]).split('\n').filter(Boolean).sort();
if (update) {
    fs.writeFileSync(manifestPath, actual.join('\n') + '\n');
    console.log(`updated ${path.relative(process.cwd(), manifestPath)} (${actual.length} files)`);
} else {
    const expected = fs.readFileSync(manifestPath, 'utf8').split('\n').filter(Boolean);
    const missing = expected.filter(f => !actual.includes(f));
    const unexpected = actual.filter(f => !expected.includes(f));
    if (missing.length || unexpected.length) {
        for (const f of missing) console.error(`missing from vsix: ${f}`);
        for (const f of unexpected) console.error(`unexpected in vsix: ${f}`);
        console.error('\nIf this change is intentional, regenerate the manifest with:');
        console.error('    npm run package && npm run check:vsix -- --update');
        process.exit(1);
    }
    console.log(`vsix manifest OK (${actual.length} files)`);
}

// 2. The bundled entry point must load with only 'vscode' stubbed.
const vscodeStub = new Proxy(function () {}, {
    get: () => vscodeStub,
    apply: () => vscodeStub,
    construct: () => vscodeStub,
});

const origLoad = Module._load;
Module._load = function (request, parent, isMain) {
    if (request === 'vscode') {
        return vscodeStub;
    }
    if (Module.isBuiltin(request)) {
        return origLoad(request, parent, isMain);
    }
    if (request.startsWith('.') || path.isAbsolute(request)) {
        return origLoad(request, parent, isMain);
    }
    throw new Error(`Unbundled dependency required at load time: ${request}`);
};

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'rossi-vsix-'));
try {
    unzip(['-q', vsixPath, '-d', tmp]);
    const ext = require(path.join(tmp, 'extension', 'dist', 'extension.js'));
    if (typeof ext.activate !== 'function') {
        throw new Error('activate() not exported by the bundled extension');
    }
    console.log('vsix smoke load OK');
} finally {
    fs.rmSync(tmp, { recursive: true, force: true });
}
