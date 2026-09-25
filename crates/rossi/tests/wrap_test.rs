//! Width-aware wrapping (`PrettyPrinter::max_line_width`).
//!
//! Pins the layout rules — operator-leading continuations hanging-aligned
//! at the (sub)formula's start column, parenthesized operands re-entered
//! one past their `(`, greedy comma fill inside brackets, quantifier
//! breaks after `·` and before `∣` — plus the safety property that a
//! wrapped element reparses as the same single AST element, and the
//! gating that keeps every preset and canonical printer flat.

mod common;

use common::{assert_reparses_equal, clear_spans, format_checked};
use rossi::{PrettyPrinter, Style, format_str, parse};

fn wrapped(width: usize) -> PrettyPrinter {
    PrettyPrinter::styled(Style::Camille).with_max_line_width(width)
}

// =========================================================================
// Chain splits
// =========================================================================

#[test]
fn conjunction_chain_splits_all_or_nothing() {
    let source =
        "MACHINE m\nINVARIANTS\n@inv1 x : NAT & y : NAT & x + y <= maximum & z : dom(f)\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let expected = "\
  @inv1 x ∈ ℕ
        ∧ y ∈ ℕ
        ∧ x + y ≤ maximum
        ∧ z ∈ dom(f)\n";
    assert!(output.contains(expected), "got:\n{output}");
}

#[test]
fn nested_operand_aligns_one_past_its_paren() {
    let source = "MACHINE m\nEVENTS\nEVENT e WHERE\n@grd1 (x = aaaaaaaaaa or y = bbbbbbbbbb or z = c) & f(y) <= bound\nTHEN @act1 x := 0 END\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let expected = "\
      @grd1 (x = aaaaaaaaaa
             ∨ y = bbbbbbbbbb
             ∨ z = c)
            ∧ f(y) ≤ bound\n";
    assert!(output.contains(expected), "got:\n{output}");
}

#[test]
fn relational_splits_and_set_literal_fills_commas() {
    let source = "CONTEXT c\nAXIOMS\n@axm1 colours = {red, green, blue, yellow, magenta}\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let expected = "\
  @axm1 colours
        = {red, green, blue, yellow,
           magenta}\n";
    assert!(output.contains(expected), "got:\n{output}");
}

#[test]
fn partition_arguments_fill_greedily() {
    let source = "CONTEXT c\nAXIOMS\n@axm2 partition(STATE, {idle}, {activated}, {closed})\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let expected = "\
  @axm2 partition(STATE, {idle},
                  {activated}, {closed})\n";
    assert!(output.contains(expected), "got:\n{output}");
}

// =========================================================================
// Assignments
// =========================================================================

#[test]
fn assignment_rhs_hangs_after_the_operator() {
    let source =
        "MACHINE m\nEVENTS\nEVENT e THEN\n@act1 x := aaaa + bbbb + cccc + dddd + eeee\nEND\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let expected = "\
      @act1 x ≔ aaaa
                + bbbb
                + cccc
                + dddd
                + eeee\n";
    assert!(output.contains(expected), "got:\n{output}");
}

#[test]
fn deep_assignment_rhs_moves_to_its_own_line() {
    // The head ends past width/2, so the right-hand side starts on a
    // fresh nested line instead of hanging absurdly deep.
    let source = "MACHINE m\nEVENTS\nEVENT e THEN\n@act1 verylongvariable := verylongvariable + somethingelse + thirdoperand\nEND\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let expected = "\
      @act1 verylongvariable ≔
              verylongvariable
              + somethingelse
              + thirdoperand\n";
    assert!(output.contains(expected), "got:\n{output}");
}

#[test]
fn wrapped_composition_value_keeps_its_guard_parens() {
    // A value with a top-level `;` must stay parenthesized so it reparses
    // as one action; the wrapped content sits one past the `(`.
    let source = "MACHINE m\nEVENTS\nEVENT e THEN\n@act1 relation := (firstrelation ; secondrelation ; thirdrelation)\nEND\nEND\n";
    let output = format_checked(source, &wrapped(40));
    // The head ends past width/2, so the parenthesized value starts on
    // its own nested line; the `;` chain aligns one past the `(`.
    let expected = "\
      @act1 relation ≔
              (firstrelation
               ; secondrelation
               ; thirdrelation)\n";
    assert!(output.contains(expected), "got:\n{output}");
}

// =========================================================================
// Quantifiers
// =========================================================================

#[test]
fn quantifier_body_breaks_after_the_dot() {
    let source = "MACHINE m\nINVARIANTS\n@inv1 !e . e : Entities & e |-> Own : rights(owner) => e = entity or Direct(e) = FALSE\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let expected = "\
  @inv1 ∀e·
          e ∈ Entities
          ∧ e ↦ Own ∈ rights(owner)
          ⇒ e = entity
            ∨ Direct(e) = FALSE\n";
    assert!(output.contains(expected), "got:\n{output}");
}

#[test]
fn comprehension_breaks_before_the_bar() {
    let source = "CONTEXT c\nAXIOMS\n@axm1 selected = {candidate . candidate : allcandidates & score(candidate) > threshold | candidate}\nEND\n";
    let output = format_checked(source, &wrapped(48));
    assert!(
        output.contains("\n           ∣ candidate}\n"),
        "bar-leading value line one past the `{{`, got:\n{output}"
    );
}

// =========================================================================
// Hang cap and stress
// =========================================================================

#[test]
fn deep_start_caps_the_hanging_column() {
    // The formula starts past width/2, so continuations fall back to the
    // element's indent plus two indent units instead of hanging there.
    let source =
        "MACHINE m\nINVARIANTS\n@a_particularly_long_label xx : NAT & yy : NAT & zz : NAT\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let expected = "\
  @a_particularly_long_label xx ∈ ℕ
      ∧ yy ∈ ℕ
      ∧ zz ∈ ℕ\n";
    assert!(output.contains(expected), "got:\n{output}");
}

#[test]
fn tiny_width_terminates_and_reparses() {
    // Atoms wider than the width overflow best-effort, so only
    // termination and reparse-equality hold here — not the width limit.
    let source = "MACHINE m\nINVARIANTS\n@inv1 somelongname : NAT & f(another) <= third + fourth\ntheorem @thm1 !x . x : S => (g(x) : T or x = fallbackvalue)\nEND\n";
    let printer = wrapped(10);
    let output = format_str(source, &printer).unwrap();
    assert_reparses_equal(source, &output);
}

#[test]
fn exact_boundary_stays_flat() {
    // `  @inv1 aa ∈ ℕ` is exactly 14 chars: flat at width 14, wrapped at 13.
    let source = "MACHINE m\nINVARIANTS\n@inv1 aa : NAT\nEND\n";
    let flat = format_checked(source, &wrapped(14));
    assert!(flat.contains("\n  @inv1 aa ∈ ℕ\n"), "got:\n{flat}");
    // At width 13 the formula's start column (8) is past width/2, so the
    // continuation falls back to the capped column (indent + 2 units = 6).
    let narrow = format_checked(source, &wrapped(13));
    assert!(
        narrow.contains("\n  @inv1 aa\n      ∈ ℕ\n"),
        "got:\n{narrow}"
    );
}

// =========================================================================
// Safety and comments
// =========================================================================

#[test]
fn a_wrapped_invariant_stays_one_element() {
    let source = "MACHINE m\nINVARIANTS\n@inv1 firstcondition : NAT & secondcondition : NAT & thirdcondition : NAT\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let rossi::Component::Machine(machine) = parse(&output).unwrap() else {
        panic!("expected machine");
    };
    assert_eq!(
        machine.invariants.len(),
        1,
        "a wrapped invariant must reparse as ONE element:\n{output}"
    );
}

#[test]
fn comment_lands_on_the_last_wrapped_line() {
    let source = "MACHINE m\nINVARIANTS\n@inv1 x : NAT & y : NAT & x + y <= maximum & z : dom(f) // stays put\nEND\n";
    let output = format_checked(source, &wrapped(40));
    assert!(
        output.contains("        ∧ z ∈ dom(f) // stays put\n"),
        "got:\n{output}"
    );
    let rossi::Component::Machine(machine) = parse(&output).unwrap() else {
        panic!("expected machine");
    };
    assert_eq!(machine.invariants[0].comment.as_deref(), Some("stays put"));
}

// =========================================================================
// Name lists and headers
// =========================================================================

#[test]
fn inline_name_list_wraps_to_the_hanging_column() {
    let source = "MACHINE m\nVARIABLES alpha beta gamma delta epsilon zeta eta theta\nINVARIANTS\n@inv1 alpha : NAT\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let expected = "\
variables alpha beta gamma delta epsilon
          zeta eta theta\n";
    assert!(output.contains(expected), "got:\n{output}");
}

#[test]
fn header_clause_moves_to_a_continuation_line() {
    let source = "MACHINE a_machine_with_a_name REFINES the_abstract_machine SEES ctx_one ctx_two\nVARIABLES x\nINVARIANTS\n@inv1 x : NAT\nEND\n";
    let output = format_checked(source, &wrapped(40));
    let expected = "\
machine a_machine_with_a_name
  refines the_abstract_machine
  sees ctx_one ctx_two\n";
    assert!(output.starts_with(expected), "got:\n{output}");
}

#[test]
fn overlong_header_clause_fills_its_own_words() {
    // A single header segment longer than the width cannot sit whole on
    // one continuation line: its names fill across continuation lines.
    let source = "MACHINE m SEES ctx_alpha ctx_beta ctx_gamma ctx_delta ctx_epsilon\nEND\n";
    let output = format_checked(source, &wrapped(30));
    let expected = "\
machine m
  sees ctx_alpha ctx_beta
  ctx_gamma ctx_delta
  ctx_epsilon\n";
    assert!(output.starts_with(expected), "got:\n{output}");
}

#[test]
fn filled_list_closer_and_comma_stay_within_width() {
    // The closing `}` after a filled list and the break comma after an
    // exactly-packed item are budgeted; without the reserve either lands
    // one column past the width (wrap_checked asserts the width).
    let source = "CONTEXT c\nAXIOMS\n@axm1 s = {aa, bb, cc, dd, ee, ff, gg}\nEND\n";
    format_checked(source, &wrapped(20));
}

// =========================================================================
// Idempotence, gating, real-world fixture
// =========================================================================

#[test]
fn wrapping_is_idempotent() {
    let source = "MACHINE m REFINES m0 SEES c1\nVARIABLES alpha beta gamma delta epsilon zeta\nINVARIANTS\n@inv1 alpha : NAT & beta : NAT & gamma + delta <= epsilon // note\nVARIANT epsilon + zeta + alpha + beta + gamma + delta\nEVENTS\nEVENT e ANY p WHERE @g p > 0 & p < epsilon + zeta THEN @a alpha := alpha + p + beta + gamma END\nEND\n";
    for width in [24, 40, 60, 120] {
        let printer = wrapped(width);
        let once = format_str(source, &printer).unwrap();
        let twice = format_str(&once, &printer).unwrap();
        assert_eq!(once, twice, "not idempotent at width {width}");
    }
}

#[test]
fn presets_and_canonical_printers_never_wrap() {
    assert_eq!(PrettyPrinter::default().max_line_width, 0);
    assert_eq!(PrettyPrinter::styled(Style::Camille).max_line_width, 0);
    assert_eq!(PrettyPrinter::styled(Style::Rossi).max_line_width, 0);
    assert_eq!(PrettyPrinter::rodin_canonical().max_line_width, 0);
    assert_eq!(PrettyPrinter::rodin_formula_string().max_line_width, 0);

    // Even a width-configured printer keeps the formula-level API flat:
    // that is what the XML and canonical paths call.
    let printer = wrapped(20);
    let source = "CONTEXT c\nAXIOMS\n@axm1 x : NAT & y : NAT & z : NAT & w : NAT\nEND\n";
    let rossi::Component::Context(ctx) = parse(source).unwrap() else {
        panic!("expected context");
    };
    let flat = printer.print_formula_predicate(&ctx.axioms[0].predicate);
    assert!(
        !flat.contains('\n'),
        "print_formula_predicate must stay flat: {flat:?}"
    );
}

#[test]
fn formula_predicate_wrapped_wraps_at_width_and_reparses() {
    let pred = rossi::parse_predicate_str(
        "element : dom(EntityNames) & EntityNames : Entities +-> POW(Containers ** Names)",
    )
    .unwrap();

    let output = wrapped(40).print_formula_predicate_wrapped(&pred);
    assert!(output.contains('\n'), "expected wrapping: {output:?}");
    for line in output.lines() {
        assert!(
            line.chars().count() <= 40,
            "line exceeds 40 chars: {line:?}"
        );
    }
    assert_eq!(
        rossi::parse_predicate_str(&output).unwrap(),
        pred,
        "wrapped output must reparse as the same predicate"
    );

    // Width 0 keeps the wrapped entry point flat, like the rest of the API.
    let flat = wrapped(0);
    assert_eq!(
        flat.print_formula_predicate_wrapped(&pred),
        flat.print_formula_predicate(&pred)
    );

    // A canonical printer stays flat even with a forced width: its output
    // feeds Rodin-canonical strings and XML attributes.
    let canonical = PrettyPrinter::rodin_canonical().with_max_line_width(30);
    let output = canonical.print_formula_predicate_wrapped(&pred);
    assert!(
        !output.contains('\n'),
        "canonical must stay flat: {output:?}"
    );
    assert_eq!(output, canonical.print_formula_predicate(&pred));
}

#[test]
fn base_model_wraps_at_120_and_reparses() {
    let source = std::fs::read_to_string("examples/base-model.eventb").unwrap();
    let printer = wrapped(120);
    let output = format_str(&source, &printer).unwrap();

    let masked = rossi::comments::lexical_spans(&output).mask_comments_chars(&output);
    for (line, masked_line) in output.lines().zip(masked.lines()) {
        if masked_line == line {
            assert!(
                line.chars().count() <= 120,
                "line exceeds 120 chars: {line:?}"
            );
        }
    }

    let mut original = rossi::parse_components(&source).unwrap();
    let mut reparsed = rossi::parse_components(&output).unwrap();
    assert_eq!(original.len(), reparsed.len());
    for (a, b) in original.iter_mut().zip(reparsed.iter_mut()) {
        clear_spans(a);
        clear_spans(b);
    }
    assert_eq!(original, reparsed, "base-model must reparse identically");
}
