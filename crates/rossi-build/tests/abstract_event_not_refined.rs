//! EB036: an abstract event the refinement leaves out. Rodin warns
//! (`AbstractEventNotRefinedWarning`) for each event of the abstract machine
//! that no event of the refinement refines, unless one of its guards is
//! literally `⊥` (`MachineEventRefinesModule.checkClosedEvents`,
//! `AbstractEventInfo.isClosed`). The cases follow Rodin's
//! `TestEventRefines`.

use rossi_build::{Diagnostic, Project, ProjectComponent, RuleId, Severity, build};

/// The EB036 findings of a build over `files` (name, text).
fn not_refined(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let components = files
        .iter()
        .flat_map(|(name, text)| {
            ProjectComponent::from_eventb(format!("{name}.eventb"), text).unwrap()
        })
        .collect();
    build(&Project::new("p", components))
        .diagnostics
        .into_iter()
        .filter(|d| d.rule_id == Some(RuleId::AbstractEventNotRefined))
        .collect()
}

/// The abstract event labels `findings` name, in order.
fn labels(findings: &[Diagnostic]) -> Vec<&str> {
    findings
        .iter()
        .map(|d| d.message.split('`').nth(1).unwrap())
        .collect()
}

const M0: &str = "\
machine M0
variables x
invariants
  @t x ∈ ℕ
events
  event INITIALISATION
    then
      @a x ≔ 0
  end
  event inc
    then
      @a x ≔ x + 1
  end
  event dec
    where
      @g 0 < x
    then
      @a x ≔ x − 1
  end
  event off
    where
      @g ⊥
    then
      @a x ≔ 0
  end
end
";

#[test]
fn an_unrefined_event_is_reported_and_a_disabled_one_is_not() {
    let m1 = "\
machine M1 refines M0
variables x
events
  event INITIALISATION extends INITIALISATION
  end
  event inc extends inc
  end
end
";
    let findings = not_refined(&[("M0", M0), ("M1", m1)]);
    assert_eq!(labels(&findings), ["dec"], "{findings:?}");
    let finding = &findings[0];
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(finding.origin, "M1");
    assert_eq!(
        finding.message,
        "abstract event `dec` of `M0` is not refined, although not disabled"
    );
}

#[test]
fn the_warning_sits_on_the_refines_clause() {
    for header in ["machine M1 refines M0", "machine M1\nrefines M0"] {
        let m1 = format!(
            "{header}\nvariables x\nevents\n  event INITIALISATION extends INITIALISATION\n  end\n  event inc extends inc\n  end\nend\n"
        );
        let findings = not_refined(&[("M0", M0), ("M1", &m1)]);
        let span = findings[0].span.expect("a span in the text");
        assert_eq!(&m1[span.start..span.end], "refines M0", "{header:?}");
    }
}

#[test]
fn a_disabling_guard_inherited_through_extends_counts() {
    let m1 = "\
machine M1 refines M0
variables x
events
  event INITIALISATION extends INITIALISATION
  end
  event inc extends inc
  end
  event dec extends dec
  end
  event off extends off
  end
end
";
    let m2 = "\
machine M2 refines M1
variables x
events
  event INITIALISATION extends INITIALISATION
  end
  event inc extends inc
  end
  event dec extends dec
  end
end
";
    let findings = not_refined(&[("M0", M0), ("M1", m1), ("M2", m2)]);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn every_target_of_a_merge_counts_as_refined() {
    let m0 = "\
machine M0
variables x
invariants
  @t x ∈ ℕ
events
  event INITIALISATION
    then
      @a x ≔ 0
  end
  event up
    where
      @g x < 5
    then
      @a x ≔ x + 1
  end
  event bump
    where
      @g 5 ≤ x
    then
      @a x ≔ x + 1
  end
end
";
    let m1 = "\
machine M1 refines M0
variables x
events
  event INITIALISATION extends INITIALISATION
  end
  event step refines up bump
    then
      @a x ≔ x + 1
  end
end
";
    let findings = not_refined(&[("M0", m0), ("M1", m1)]);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn an_event_of_the_same_name_without_refines_is_reported() {
    let m1 = "\
machine M1 refines M0
variables x
events
  event INITIALISATION extends INITIALISATION
  end
  event inc extends inc
  end
  event dec
  end
end
";
    let findings = not_refined(&[("M0", M0), ("M1", m1)]);
    assert_eq!(labels(&findings), ["dec"], "{findings:?}");
}

#[test]
fn a_refinement_without_initialisation_is_reported_for_it() {
    let m1 = "\
machine M1 refines M0
variables x
events
  event inc extends inc
  end
  event dec extends dec
  end
end
";
    let findings = not_refined(&[("M0", M0), ("M1", m1)]);
    assert_eq!(labels(&findings), ["INITIALISATION"], "{findings:?}");
}

#[test]
fn an_event_dropped_for_an_unknown_target_still_refines_the_others() {
    let m1 = "\
machine M1 refines M0
variables x
events
  event INITIALISATION extends INITIALISATION
  end
  event inc extends inc
  end
  event down refines dec nowhere
    then
      @a x ≔ x − 1
  end
end
";
    let findings = not_refined(&[("M0", M0), ("M1", m1)]);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn only_the_direct_abstraction_is_read() {
    let m1 = "\
machine M1 refines M0
variables x
events
  event INITIALISATION extends INITIALISATION
  end
  event inc extends inc
  end
end
";
    let m2 = "\
machine M2 refines M1
variables x
events
  event INITIALISATION extends INITIALISATION
  end
  event inc extends inc
  end
end
";
    let findings = not_refined(&[("M0", M0), ("M1", m1), ("M2", m2)]);
    let origins: Vec<&str> = findings.iter().map(|d| d.origin.as_str()).collect();
    assert_eq!(origins, ["M1"], "{findings:?}");
}
