//! The obligation index over generated `.bpo` files: every obligation the
//! generator wrote is listed with its nature, and loads as a sequent.

mod common;

use rossi::formula::{PredicateKind, tag};
use rossi_build::ScFile;
use rossi_build::po_view::PoView;
use rossi_build::pog::obligations::{ObligationStatus, Obligations};
use rossi_build::project::discover_projects;
use rossi_prove::confidence::Bucket;

const MODELS: &[&str] = &[
    "base-model",
    "binary-search",
    "cars-on-bridge",
    "file-system",
    "traffic-light",
];

/// The files a build of one example archive emits.
fn build_files(model: &str) -> Vec<ScFile> {
    let zip = common::workspace_root()
        .join("crates/rossi/examples")
        .join(format!("{model}.zip"));
    let bytes = std::fs::read(&zip).unwrap_or_else(|e| panic!("read {}: {e}", zip.display()));
    let projects =
        discover_projects(&bytes, model).unwrap_or_else(|e| panic!("{model}: discovery: {e}"));
    projects
        .into_iter()
        .flat_map(|dp| rossi_build::build(&dp.into_project()).files)
        .collect()
}

#[test]
fn every_generated_obligation_is_listed_with_a_nature_and_loads() {
    for model in MODELS {
        let files = build_files(model);
        let index = Obligations::from_files(&files).unwrap_or_else(|e| panic!("{model}: {e}"));

        let expected: usize = files
            .iter()
            .filter(|f| f.filename.ends_with(".bpo"))
            .map(|f| PoView::from_xml(&f.contents).unwrap().sequents.len())
            .sum();
        assert!(expected > 0, "{model}: no obligations generated");
        let listed: Vec<_> = index.iter().collect();
        assert_eq!(listed.len(), expected, "{model}: obligation count");

        for po in &listed {
            assert!(
                po.nature.is_some(),
                "{model}: {}/{} has an unknown description {:?}",
                po.component,
                po.name,
                po.description
            );
            assert_eq!(po.nature.map(|n| n.description()), Some(po.description));
            let sequent = index
                .sequent(po.component, po.name)
                .unwrap_or_else(|e| panic!("{model}: {}/{}: {e}", po.component, po.name));
            // A fresh build records every obligation as unattempted at
            // its own stamp.
            assert_eq!(
                po.status,
                Some(ObligationStatus {
                    bucket: Bucket::Unattempted,
                    confidence: Some(-99),
                    broken: false,
                    manual: false,
                    stale: false,
                }),
                "{model}: {}/{}",
                po.component,
                po.name
            );
            // The generator suppresses trivial goals, so a loaded goal
            // is never `⊤`.
            assert!(
                !matches!(
                    sequent.goal().kind(),
                    PredicateKind::Literal(tag::LiteralPredOp::BTrue)
                ),
                "{model}: {}/{} loaded a trivial goal",
                po.component,
                po.name
            );
        }

        let first = listed[0];
        let found = index
            .get(first.component, first.name)
            .expect("lookup by name");
        assert_eq!((found.component, found.name), (first.component, first.name));
        assert!(index.get(first.component, "no/such/PO").is_none());
        assert!(index.sequent("no_such_component", first.name).is_err());
    }
}
