import { ExtensionContext, OutputChannel, ProgressLocation, window, workspace } from 'vscode';
import { execFile } from 'child_process';
import * as crypto from 'crypto';
import * as fs from 'fs/promises';
import * as https from 'https';
import * as path from 'path';
import { URL } from 'url';

// Resolved absolute paths (or PATH-resolvable names) for the two binaries the
// extension drives.
export interface ResolvedBinaries {
    languageServer: string;
    cli: string;
}

const LS_NAME = 'eventb-language-server';
const CLI_NAME = 'rossi';

// The release host. Matches the `repository` in package.json / Cargo.toml; the
// prebuilt assets are attached to each GitHub Release by .github/workflows/release.yml.
const REPO = 'eventb-rossi/rossi';
const RELEASES = `https://github.com/${REPO}/releases`;
const USER_AGENT = 'eventb-vscode';

// `${process.platform}-${process.arch}` -> Rust target triple. Mirrors the
// release workflow's build matrix.
const TARGET_TRIPLES: Record<string, string> = {
    'win32-x64': 'x86_64-pc-windows-msvc',
    'win32-arm64': 'aarch64-pc-windows-msvc',
    'darwin-x64': 'x86_64-apple-darwin',
    'darwin-arm64': 'aarch64-apple-darwin',
    'linux-x64': 'x86_64-unknown-linux-gnu',
    'linux-arm64': 'aarch64-unknown-linux-gnu',
};

/**
 * Resolve both binaries. Per binary the order is: an explicit user-configured
 * path, then a copy on PATH (the from-source developer workflow), then a copy
 * downloaded from the matching GitHub Release into the extension's global
 * storage. The download happens at most once — both binaries ship in one
 * archive — and only when at least one binary is missing locally.
 */
export async function resolveBinaries(
    context: ExtensionContext,
    output: OutputChannel
): Promise<ResolvedBinaries> {
    const config = workspace.getConfiguration('rossi');
    const languageServer = await resolveLocal(config.get<string>('languageServer.path', LS_NAME).trim(), LS_NAME);
    const cli = await resolveLocal(config.get<string>('tool.path', CLI_NAME).trim(), CLI_NAME);
    if (languageServer && cli) {
        return { languageServer, cli };
    }

    const downloaded = await ensureDownloaded(context, output);
    return {
        languageServer: languageServer ?? downloaded.languageServer,
        cli: cli ?? downloaded.cli,
    };
}

// An explicit path (absolute or containing a separator) must exist; otherwise
// treat the value as a command name and search PATH. Returns undefined when a
// bare name is not on PATH, signalling that a download is needed.
async function resolveLocal(configured: string, defaultName: string): Promise<string | undefined> {
    if (configured && configured !== defaultName && looksLikePath(configured)) {
        if (await isFile(configured)) {
            return configured;
        }
        throw new Error(`Configured path does not exist: ${configured}`);
    }
    return findOnPath(configured || defaultName);
}

async function ensureDownloaded(
    context: ExtensionContext,
    output: OutputChannel
): Promise<ResolvedBinaries> {
    const { triple, ext } = currentTarget();
    const version = String(context.extension.packageJSON.version);
    const cacheDir = path.join(context.globalStorageUri.fsPath, 'bin', version, triple);
    const result = binariesIn(cacheDir);

    if (await isFile(result.languageServer) && await isFile(result.cli)) {
        return result;
    }

    // Download and extract into a process-private staging directory, then move it
    // into place atomically. Two editor windows can reach here at once; whichever
    // renames first wins and the loser discards its copy, so the cache is never a
    // half-extracted archive.
    const staging = `${cacheDir}.tmp.${process.pid}`;
    const staged = binariesIn(staging);
    await fs.rm(staging, { recursive: true, force: true });
    await window.withProgress(
        { location: ProgressLocation.Notification, title: 'Rossi: downloading the Event-B toolchain…' },
        () => downloadAndExtract(version, triple, ext, staging, output)
    );

    if (!(await isFile(staged.languageServer)) || !(await isFile(staged.cli))) {
        await fs.rm(staging, { recursive: true, force: true });
        throw new Error('the downloaded archive did not contain the expected binaries');
    }
    if (process.platform !== 'win32') {
        await fs.chmod(staged.languageServer, 0o755);
        await fs.chmod(staged.cli, 0o755);
    }

    await fs.mkdir(path.dirname(cacheDir), { recursive: true });
    try {
        await fs.rename(staging, cacheDir);
    } catch {
        // Another window populated the cache first (or a stale dir is in the way);
        // drop our copy and rely on the existence check below.
        await fs.rm(staging, { recursive: true, force: true });
    }
    if (!(await isFile(result.languageServer)) || !(await isFile(result.cli))) {
        throw new Error('failed to install the downloaded binaries into the cache');
    }
    return result;
}

async function downloadAndExtract(
    version: string,
    triple: string,
    ext: string,
    destDir: string,
    output: OutputChannel
): Promise<void> {
    const assetName = `rossi-${triple}.${ext}`;

    // Pin to the tag matching the extension version so the server and client are
    // in lock-step; fall back to the latest release if that tag has no assets
    // (e.g. the extension was bumped ahead of a binary release).
    let tag = `v${version}`;
    let sums = await fetchFollowingRedirects(`${RELEASES}/download/${tag}/SHA256SUMS`);
    if (sums.status === 404) {
        tag = await resolveLatestTag();
        output.appendLine(`Rossi: no release for v${version}; using the latest release (${tag}).`);
        sums = await fetchFollowingRedirects(`${RELEASES}/download/${tag}/SHA256SUMS`);
    }
    if (sums.status !== 200) {
        throw new Error(`could not fetch SHA256SUMS for ${tag} (HTTP ${sums.status})`);
    }

    const expected = parseChecksum(sums.body.toString('utf8'), assetName);
    if (!expected) {
        throw new Error(`SHA256SUMS for ${tag} has no entry for ${assetName}`);
    }

    const assetUrl = `${RELEASES}/download/${tag}/${assetName}`;
    output.appendLine(`Rossi: downloading ${assetUrl}`);
    const archive = await fetchFollowingRedirects(assetUrl);
    if (archive.status !== 200) {
        throw new Error(`failed to download ${assetName} (HTTP ${archive.status})`);
    }

    const actual = crypto.createHash('sha256').update(archive.body).digest('hex');
    if (actual !== expected) {
        throw new Error(`checksum mismatch for ${assetName}: expected ${expected}, got ${actual}`);
    }

    await fs.mkdir(destDir, { recursive: true });
    const archivePath = path.join(destDir, assetName);
    await fs.writeFile(archivePath, archive.body);
    try {
        // `tar` ships on Linux, macOS, and Windows 10 1803+ (bsdtar, which also
        // reads .zip), so one extraction path covers every platform without a
        // bundled archive library.
        const args = ext === 'zip'
            ? ['-xf', archivePath, '-C', destDir]
            : ['-xzf', archivePath, '-C', destDir];
        await run('tar', args);
    } finally {
        await fs.rm(archivePath, { force: true });
    }
}

async function resolveLatestTag(): Promise<string> {
    const res = await fetchFollowingRedirects(`https://api.github.com/repos/${REPO}/releases/latest`);
    if (res.status !== 200) {
        throw new Error(`could not query the latest release (HTTP ${res.status})`);
    }
    const data = JSON.parse(res.body.toString('utf8')) as { tag_name?: string };
    if (!data.tag_name) {
        throw new Error('the latest release has no tag');
    }
    return data.tag_name;
}

function currentTarget(): { triple: string; ext: string } {
    const triple = TARGET_TRIPLES[`${process.platform}-${process.arch}`];
    if (!triple) {
        throw new Error(
            `no prebuilt binary for ${process.platform}/${process.arch}; build from source (see the extension's INSTALL guide)`
        );
    }
    return { triple, ext: process.platform === 'win32' ? 'zip' : 'tar.gz' };
}

// Pick out the lower-cased hash for `name` from `sha256sum`-format text
// (`<hash>  <filename>` per line).
function parseChecksum(text: string, name: string): string | undefined {
    for (const line of text.split(/\r?\n/)) {
        const parts = line.trim().split(/\s+/);
        if (parts.length >= 2 && parts[parts.length - 1] === name) {
            return parts[0].toLowerCase();
        }
    }
    return undefined;
}

interface HttpResult {
    status: number;
    body: Buffer;
}

// GitHub release-asset URLs redirect to a CDN, so follow 3xx Location headers
// manually (the bundled `https` module does not). Used for small text (the
// checksum manifest, the releases API) and the binary archive alike.
async function fetchFollowingRedirects(url: string): Promise<HttpResult> {
    let current = url;
    for (let hop = 0; hop < 6; hop++) {
        const res = await httpGet(current);
        if ((res.status === 301 || res.status === 302 || res.status === 307 || res.status === 308) && res.location) {
            // Resolve relative `Location` headers against the URL we just fetched.
            current = new URL(res.location, current).toString();
            continue;
        }
        return { status: res.status, body: res.body };
    }
    throw new Error(`too many redirects fetching ${url}`);
}

function httpGet(url: string): Promise<{ status: number; location?: string; body: Buffer }> {
    return new Promise((resolve, reject) => {
        const request = https.get(url, { headers: { 'User-Agent': USER_AGENT }, timeout: 30_000 }, (response) => {
            const chunks: Buffer[] = [];
            response.on('data', (chunk: Buffer) => chunks.push(chunk));
            response.on('end', () => resolve({
                status: response.statusCode ?? 0,
                location: response.headers.location,
                body: Buffer.concat(chunks),
            }));
            response.on('error', reject);
        });
        // `timeout` only arms the socket-idle timer; destroy explicitly so a
        // stalled connection rejects instead of hanging extension activation.
        request.on('timeout', () => request.destroy(new Error(`request to ${url} timed out`)));
        request.on('error', reject);
    });
}

function run(command: string, args: string[]): Promise<void> {
    return new Promise((resolve, reject) => {
        execFile(command, args, (error) => (error ? reject(error) : resolve()));
    });
}

async function findOnPath(command: string): Promise<string | undefined> {
    const dirs = (process.env.PATH ?? '').split(path.delimiter).filter(Boolean);
    const extensions = process.platform === 'win32'
        ? (process.env.PATHEXT ?? '.EXE;.CMD;.BAT;.COM').split(';').filter(Boolean)
        : [''];
    for (const dir of dirs) {
        for (const extension of extensions) {
            const candidate = path.join(dir, command + extension.toLowerCase());
            if (await isFile(candidate)) {
                return candidate;
            }
        }
    }
    return undefined;
}

function looksLikePath(value: string): boolean {
    return value.includes('/') || value.includes('\\') || path.isAbsolute(value);
}

async function isFile(target: string): Promise<boolean> {
    try {
        return (await fs.stat(target)).isFile();
    } catch {
        return false;
    }
}

function exeName(name: string): string {
    return process.platform === 'win32' ? `${name}.exe` : name;
}

// The two binaries' paths inside a directory (the cache dir or a staging dir).
function binariesIn(dir: string): ResolvedBinaries {
    return {
        languageServer: path.join(dir, exeName(LS_NAME)),
        cli: path.join(dir, exeName(CLI_NAME)),
    };
}
