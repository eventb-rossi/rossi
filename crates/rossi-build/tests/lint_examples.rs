//! The bundled example archives are lint-clean end to end: their kept
//! variables' only cross-level uses are inherited invariants and
//! extended-event clauses, which the lint folds in the same way the SC
//! materialises them into the `.bcm`.

use rossi_build::{Project, lint};

#[test]
fn bundled_examples_are_lint_clean() {
    for name in ["binary-search", "traffic-light"] {
        let path = format!("../rossi/examples/{name}.zip");
        let project = Project::from_zip_file(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let diags = lint::run(&project);
        assert!(diags.is_empty(), "{name} should be lint-clean: {diags:#?}");
    }
}
