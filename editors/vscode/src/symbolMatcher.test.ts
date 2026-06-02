/**
 * Standalone unit tests for the pure symbol matcher. No VSCode required.
 *
 *   npm run test:matcher
 *
 * Exits non-zero on the first failure so it can gate CI / pre-commit.
 */
import {
    OperatorRow,
    SymbolMatcher,
    simulateEagerTyping,
    leaderTokenBefore,
} from './symbolMatcher';

// A representative slice of the real `rossi/operatorTable` payload. Covers the
// ambiguous prefixes, an alphabetic op, and a backslash op (leader-only).
const ROWS: OperatorRow[] = [
    { ascii: '&', unicode: '∧', description: 'and', aliases: ['and'], symbolic: true, eager: true },
    { ascii: '=>', unicode: '⇒', description: 'implies', aliases: ['implies', 'imp'], symbolic: true, eager: true },
    { ascii: '<=>', unicode: '⇔', description: 'iff', aliases: ['iff'], symbolic: true, eager: true },
    { ascii: '<=', unicode: '≤', description: 'le', aliases: ['le', 'leq'], symbolic: true, eager: true },
    { ascii: '|->', unicode: '↦', description: 'maplet', aliases: ['maplet'], symbolic: true, eager: true },
    { ascii: '|>', unicode: '▷', description: 'ranres', aliases: [], symbolic: true, eager: true },
    { ascii: '|>>', unicode: '⩥', description: 'ransub', aliases: [], symbolic: true, eager: true },
    { ascii: ':=', unicode: '≔', description: 'assign', aliases: ['assign'], symbolic: true, eager: true },
    { ascii: ':', unicode: '∈', description: 'in', aliases: ['in'], symbolic: true, eager: true },
    { ascii: '::', unicode: ':∈', description: 'becomesin', aliases: [], symbolic: true, eager: true },
    { ascii: '<:', unicode: '⊆', description: 'subseteq', aliases: ['subseteq'], symbolic: true, eager: true },
    { ascii: '/=', unicode: '≠', description: 'neq', aliases: ['neq'], symbolic: true, eager: true },
    // Symbolic but NOT eager (server policy): bare `/` and backslash ops.
    { ascii: '/', unicode: '÷', description: 'divide', aliases: ['div'], symbolic: true, eager: false },
    { ascii: '..', unicode: '‥', description: 'range', aliases: [], symbolic: true, eager: true },
    { ascii: '\\/', unicode: '∪', description: 'union', aliases: ['union', 'cup'], symbolic: true, eager: false },
    { ascii: 'NAT', unicode: 'ℕ', description: 'nat', aliases: ['nat'], symbolic: false, eager: false },
    { ascii: 'or', unicode: '∨', description: 'or', aliases: ['or'], symbolic: false, eager: false },
];

const matcher = new SymbolMatcher(ROWS);

let failures = 0;
function check(label: string, got: unknown, want: unknown): void {
    const ok = JSON.stringify(got) === JSON.stringify(want);
    if (!ok) {
        failures++;
        console.error(`✗ ${label}\n    got:  ${JSON.stringify(got)}\n    want: ${JSON.stringify(want)}`);
    } else {
        console.log(`✓ ${label}`);
    }
}

// --- Eager substitution -----------------------------------------------------

// Single-char complete op converts immediately on keystroke.
check('& -> and', simulateEagerTyping(matcher, 'a&b'), 'a∧b');
// Two-char op completes on its final char.
check('=> -> implies', simulateEagerTyping(matcher, 'x=>y'), 'x⇒y');
// Prefix ambiguity: <= is held until a boundary proves no `>` follows.
check('<= waits then converts on boundary', simulateEagerTyping(matcher, 'a<=b'), 'a≤b');
check('<=> beats <=', simulateEagerTyping(matcher, 'p<=>q'), 'p⇔q');
// Maplet is built up across three chars.
check('|-> -> maplet', simulateEagerTyping(matcher, 'f|->x'), 'f↦x');
// |> held (because of |>>) then converts on boundary; |>> wins when completed.
check('|> waits then converts', simulateEagerTyping(matcher, 'a|>b'), 'a▷b');
check('|>> beats |>', simulateEagerTyping(matcher, 'a|>>b'), 'a⩥b');
// Assignment vs membership disambiguation.
check(':= -> assign', simulateEagerTyping(matcher, 'x:=y'), 'x≔y');
check(': -> in on boundary', simulateEagerTyping(matcher, 'x:y'), 'x∈y');
check(':: -> becomes-in', simulateEagerTyping(matcher, 'x::y'), 'x:∈y');
check('<: -> subseteq', simulateEagerTyping(matcher, 'S<:T'), 'S⊆T');

// Blocklisted single chars stay literal; their extensions still convert.
check('// stays a comment', simulateEagerTyping(matcher, '// a=>b'), '// a⇒b');
check('lone / stays', simulateEagerTyping(matcher, 'a/b'), 'a/b');
check('/= still converts', simulateEagerTyping(matcher, 'a/=b'), 'a≠b');
check('.. still converts', simulateEagerTyping(matcher, '1..5'), '1‥5');
check('lone . stays', simulateEagerTyping(matcher, 'a.b'), 'a.b');

// Backslash is reserved for the leader: \/ (union) is NOT eager, and a
// backslash always ends the current run without converting it.
check('\\ does not eager-convert', simulateEagerTyping(matcher, 'a\\/b'), 'a\\/b');
check('\\ cancels a held run', simulateEagerTyping(matcher, '<\\x'), '<\\x');

// Alphabetic ops are never eager.
check('NAT stays literal', simulateEagerTyping(matcher, 'x:NAT'), 'x∈NAT');
check('or stays literal', simulateEagerTyping(matcher, 'p or q'), 'p or q');

// --- Leader resolution ------------------------------------------------------

check('\\and', matcher.resolveLeader('and'), '∧');
check('\\to is absent here', matcher.resolveLeader('to'), null);
check('\\nat (alias)', matcher.resolveLeader('nat'), 'ℕ');
check('\\NAT (ascii word)', matcher.resolveLeader('NAT'), 'ℕ');
check('\\or (ascii word)', matcher.resolveLeader('or'), '∨');
check('\\union', matcher.resolveLeader('union'), '∪');
check('unknown leader', matcher.resolveLeader('nope'), null);

// --- Leader token scanning --------------------------------------------------

check('token before cursor', leaderTokenBefore('foo \\foral'), { name: 'foral', start: 4 });
check('no token (space after)', leaderTokenBefore('\\and '), null);
check('no backslash', leaderTokenBefore('and'), null);
check('token at start', leaderTokenBefore('\\in'), { name: 'in', start: 0 });

if (failures > 0) {
    console.error(`\n${failures} assertion(s) failed`);
    process.exit(1);
}
console.log('\nAll symbol matcher tests passed');
