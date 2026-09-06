/**
 * CLI style flags mirroring the `rossi.format.*` settings, so commands that
 * shell out to `rossi fmt` format the same way as the LSP formatter. Tolerant
 * like the server's config parsing (crates/eventb-lsp/src/config.rs): enum
 * strings are matched case-insensitively and an unknown or mistyped value
 * falls back to the style preset by omitting the flag. `useUnicode` is
 * deliberately not mirrored — the convert commands pass an explicit
 * `--ascii`/`--unicode`. `privateUseGlyphs` is mirrored, because nothing else
 * in the invocation carries it and `rossi fmt` would otherwise write a
 * different spelling of `<+` than the LSP formatter.
 */
export interface FormatSettings {
    style: unknown;
    keywordCase: unknown;
    declLists: unknown;
    blankBetweenClauses: unknown;
    indentation: unknown;
    maxLineWidth: unknown;
    privateUseGlyphs: unknown;
}

function pick(value: unknown, allowed: readonly string[]): string | undefined {
    if (typeof value !== 'string') {
        return undefined;
    }
    const normalized = value.trim().toLowerCase();
    return allowed.includes(normalized) ? normalized : undefined;
}

export function formatStyleFlags(format: FormatSettings): string[] {
    const flags: string[] = [];
    const style = pick(format.style, ['camille', 'rossi']);
    if (style) {
        flags.push('--style', style);
    }
    const keywordCase = pick(format.keywordCase, ['lower', 'upper']);
    if (keywordCase) {
        flags.push('--keyword-case', keywordCase);
    }
    const declLists = pick(format.declLists, ['inline', 'one-per-line']);
    if (declLists) {
        flags.push('--decl-lists', declLists);
    }
    if (typeof format.blankBetweenClauses === 'boolean') {
        flags.push('--blank-between-clauses', String(format.blankBetweenClauses));
    }
    if (typeof format.indentation === 'string' && format.indentation !== '') {
        flags.push('--indent', format.indentation);
    }
    if (
        typeof format.maxLineWidth === 'number' &&
        Number.isInteger(format.maxLineWidth) &&
        format.maxLineWidth >= 0
    ) {
        flags.push('--max-width', String(format.maxLineWidth));
    }
    if (format.privateUseGlyphs === true) {
        flags.push('--private-use-glyphs');
    }
    return flags;
}
