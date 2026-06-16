/**
 * Pure conversion of a `rossi validate --format json` source region to the
 * coordinates VS Code Positions use. No VSCode dependency, so it is unit-tested
 * standalone (see `validationRegion.test.ts`).
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
