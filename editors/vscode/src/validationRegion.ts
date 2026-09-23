/**
 * Pure placement of a `rossi validate --format json` finding: its source region
 * in the coordinates VS Code Positions use, and whether the language server
 * already shows it there. No VSCode dependency, so it is unit-tested standalone
 * (see `validationRegion.test.ts`).
 */

/** A 1-indexed source region as emitted by `rossi validate` (SARIF convention). */
export interface ValidationRegion {
    start_line: number;
    start_column: number;
    end_line: number;
    end_column: number;
}

/** A 0-indexed line/character span — the VS Code `Position` convention. */
export interface ZeroIndexedRange {
    startLine: number;
    startChar: number;
    endLine: number;
    endChar: number;
}

/**
 * Convert a validate region (1-indexed) to 0-indexed VS Code coordinates.
 *
 * Falls back to a one-character span at the start of the file when no region is
 * known — diagnostics on Rodin-XML-sourced components, or project-level
 * findings, carry no position. (This is the same place the diagnostic used to
 * be pinned unconditionally, before regions were wired through.)
 */
export function regionToZeroIndexed(region: ValidationRegion | undefined): ZeroIndexedRange {
    if (!region) {
        return { startLine: 0, startChar: 0, endLine: 0, endChar: 1 };
    }
    const zero = (oneIndexed: number) => Math.max(0, oneIndexed - 1);
    return {
        startLine: zero(region.start_line),
        startChar: zero(region.start_column),
        endLine: zero(region.end_line),
        endChar: zero(region.end_column),
    };
}

/** A diagnostic the language server shows for a file, reduced to what matching needs. */
export interface ShownDiagnostic {
    /** 0-indexed start line. */
    line: number;
    code?: string;
    isError: boolean;
}

/**
 * Whether the language server already shows the finding validate reports as
 * `ruleId` at `region`, so adding it would list it twice.
 *
 * The match is a diagnostic with the same code starting on the same line.
 * Neither the message nor the column is compared: validate's text is the
 * server's with a location prefix, some rules are anchored on another token of
 * the line, and columns count characters where the server counts UTF-16
 * units. EB004 is the CLI's catch-all for a syntax error, which the server
 * reports as an error with no code. A finding without a rule or a region, such
 * as a proof status or a project-level check, is never matched.
 */
export function shownByServer(
    ruleId: string | undefined,
    region: ValidationRegion | undefined,
    shown: readonly ShownDiagnostic[]
): boolean {
    if (!ruleId || !region) {
        return false;
    }
    const line = regionToZeroIndexed(region).startLine;
    return shown.some(
        (diagnostic) =>
            diagnostic.line === line &&
            (diagnostic.code === ruleId ||
                (ruleId === 'EB004' && diagnostic.code === undefined && diagnostic.isError))
    );
}
