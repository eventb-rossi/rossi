//! Collect the clicked machine's dependency closure from live buffers,
//! statically check it in memory, and write a throwaway Rodin project.
//!
//! The closure walk prefers open buffers over disk (unsaved edits are
//! animated) and follows the *parsed* dependency clauses — REFINES/SEES on
//! machines, EXTENDS on contexts — rather than the workspace graph, which
//! trails the diagnostics debounce. Every closure member must parse cleanly:
//! a recovered AST silently drops elements, and animating a model with
//! dropped elements produces verdicts about a different model.

use std::collections::HashSet;
use std::path::Path;

use crate::component_loader::ComponentLoader;
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::lsp_types::Uri;

use super::{AnimateError, AnimateMode};

/// The Rodin project name every temp build uses.
const PROJECT_NAME: &str = "rossi_animate";

/// One labeled invariant declared somewhere in the closure, with the
/// renderings used to match the tool's printed predicates back to it.
#[derive(Debug)]
pub(crate) struct InvariantInfo {
    pub label: String,
    /// The machine that declares it.
    pub component: String,
    /// The file that held that machine when the run started.
    pub uri: Uri,
    /// Whitespace-stripped renderings (Rodin-canonical and ASCII) of the
    /// predicate; the tool prints the `.bcm` predicate code, which our
    /// build derives from the same AST.
    pub renderings: Vec<String>,
}

/// The structural facts PO-name anchoring needs about one closure member.
/// (Invariant labels are answered by [`Closure::invariants`], the single
/// record of label declarations.)
#[derive(Debug)]
pub(crate) struct ComponentInfo {
    pub name: String,
    pub uri: Uri,
    /// Event names, including `INITIALISATION` when present.
    pub event_names: HashSet<String>,
}

/// The dependency closure of the clicked machine, ready for the pipeline.
#[derive(Debug)]
pub(crate) struct Closure {
    /// Name of the clicked machine.
    pub machine: String,
    /// The clicked file (diagnostic fallback anchor).
    pub uri: Uri,
    /// All components (machine, refinement ancestors, visible contexts).
    pub components: Vec<rossi::NamedComponent>,
    /// Every labeled invariant declared across the closure's machines.
    pub invariants: Vec<InvariantInfo>,
    /// Per-component structure for PO-name anchoring.
    pub infos: Vec<ComponentInfo>,
}

/// Everything [`super::execute`]'s blocking stage produces: the closure, the
/// temp project the tool runs on (removed when the guard drops), and the
/// clicked machine's proof-obligation count for the po watchdog — all
/// generated sequents in Check mode, the still-open ones after merging
/// recorded proof state in Po mode.
#[derive(Debug)]
pub(crate) struct Prepared {
    pub closure: Closure,
    pub temp_dir: tempfile::TempDir,
    pub po_count: usize,
}

/// The full blocking stage: closure → static check → (Po mode) recorded
/// proof state → temp project.
pub(crate) fn prepare(
    cross_references: &CrossReferenceManager,
    documents: &DocumentManager,
    uri: &Uri,
    machine: &str,
    mode: AnimateMode,
    rodin_project_dir: Option<&Path>,
) -> Result<Prepared, AnimateError> {
    let closure = collect_closure(cross_references, documents, uri, machine)?;
    let mut build = build_in_memory(&closure)?;
    let po_count = match mode {
        AnimateMode::Po => apply_recorded_proof_state(&mut build, &closure, rodin_project_dir),
        AnimateMode::Check => count_po_sequents(&build, &closure.machine),
    };
    let temp_dir = write_temp_project(&closure.components, &build)?;
    Ok(Prepared {
        closure,
        temp_dir,
        po_count,
    })
}

/// Po mode only: merge recorded Rodin proof state into the generated
/// `.bpo`/`.bps` pairs so obligations Rodin already discharged skip the
/// disprover, reset stamp-diverged statuses (the po gate is stamp-blind, so
/// a stale proof of a since-edited obligation must never pass vacuously),
/// and return the clicked machine's open-obligation count for the watchdog.
///
/// Check mode deliberately skips this: with recorded statuses present,
/// ProB's `PROOF_INFO` would let discharged INV obligations skip invariant
/// re-checking during the model check.
fn apply_recorded_proof_state(
    build: &mut rossi_build::BuildResult,
    closure: &Closure,
    rodin_project_dir: Option<&Path>,
) -> usize {
    rossi_build::pog::reconcile::reconcile_build_files(&mut build.files, |name| {
        recorded_proof_file(name, rodin_project_dir, closure)
    });
    let bpo_name = format!("{}.bpo", closure.machine);
    rossi_build::pog::reconcile::reset_stale_statuses(&mut build.files)
        .into_iter()
        .find(|(name, _)| *name == bpo_name)
        .map_or(0, |(_, open)| open)
}

/// Previously recorded contents for generated proof file `name`
/// ("M0.bpo" / "M0.bps"), freshest source first:
/// 1. the shared Rodin workspace project (Rodin writes `.bps` there live
///    while a session runs);
/// 2. the component's own source directory (the proof mirror's session-end
///    checkout copies, or a plain Rodin export);
/// 3. `None` — reconcile no-ops for the pair and every obligation stays
///    unattempted, the pre-existing behavior.
///
/// Workspace-first is deliberately the *inverse* of `rodin::proof_mirror`'s
/// seed policy ("the checkout is authoritative when a session starts"): the
/// mirror arbitrates ownership at session boundaries, while this lens wants
/// whatever state is freshest right now.
fn recorded_proof_file(
    name: &str,
    rodin_project_dir: Option<&Path>,
    closure: &Closure,
) -> Option<String> {
    if !rossi_build::is_normal_path_component(name) {
        return None;
    }
    if let Some(dir) = rodin_project_dir
        && let Ok(contents) = std::fs::read_to_string(dir.join(name))
    {
        return Some(contents);
    }
    let stem = name
        .strip_suffix(".bpo")
        .or_else(|| name.strip_suffix(".bps"))?;
    let info = closure.infos.iter().find(|info| info.name == stem)?;
    let path = info.uri.to_file_path()?;
    std::fs::read_to_string(path.parent()?.join(name)).ok()
}

/// Collect the machine named `machine` in the document at `uri`, plus its
/// refinement ancestors and visible contexts (transitively, EXTENDS
/// included), preferring open buffers over disk.
///
/// Names declared in the clicked file win over same-named components
/// elsewhere in the workspace; the workspace index resolves the rest, with a
/// sibling `<name>.eventb` / `<name>.txt` fallback for single-file sessions
/// whose index was never scanned.
pub(crate) fn collect_closure(
    cross_references: &CrossReferenceManager,
    documents: &DocumentManager,
    uri: &Uri,
    machine: &str,
) -> Result<Closure, AnimateError> {
    let loader = ComponentLoader::new(cross_references, Some(documents));
    let clicked = loader
        .parsed(uri)
        .ok_or_else(|| AnimateError::SourceUnavailable(uri.as_str().to_owned()))?;
    ensure_clean_parse(&clicked, machine)?;
    let component = clicked
        .components()
        .iter()
        .find(|c| c.name() == machine)
        .ok_or_else(|| AnimateError::MissingComponent(machine.to_string()))?;
    if !matches!(component, rossi::Component::Machine(_)) {
        return Err(AnimateError::NotAMachine(machine.to_string()));
    }

    let mut closure = Closure {
        machine: machine.to_string(),
        uri: uri.clone(),
        components: Vec::new(),
        invariants: Vec::new(),
        infos: Vec::new(),
    };
    let mut seen = HashSet::from([machine.to_string()]);
    let mut queue = record_component(component, uri, &mut closure);

    while let Some(name) = queue.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let (doc, dep_uri) = resolve_dependency(&loader, &clicked, uri, &name)?;
        ensure_clean_parse(&doc, &name)?;
        let component = doc
            .components()
            .iter()
            .find(|c| c.name() == name)
            .ok_or_else(|| AnimateError::MissingComponent(name.clone()))?;
        queue.extend(record_component(component, &dep_uri, &mut closure));
    }
    Ok(closure)
}

/// A recovered parse is not good enough here — see the module docs.
fn ensure_clean_parse(
    doc: &crate::document::ParsedDocument,
    name: &str,
) -> Result<(), AnimateError> {
    if doc.parse().errors.is_empty() {
        Ok(())
    } else {
        Err(AnimateError::ParseFailed(name.to_string()))
    }
}

/// Resolve a dependency to its parsed document: the clicked file first (its
/// declarations always win), then the workspace index / open buffers, then
/// sibling files next to the clicked one.
fn resolve_dependency(
    loader: &ComponentLoader<'_>,
    clicked: &std::sync::Arc<crate::document::ParsedDocument>,
    clicked_uri: &Uri,
    name: &str,
) -> Result<(std::sync::Arc<crate::document::ParsedDocument>, Uri), AnimateError> {
    if clicked.components().iter().any(|c| c.name() == name) {
        return Ok((std::sync::Arc::clone(clicked), clicked_uri.clone()));
    }
    if let Some(loaded) = loader.load(name) {
        let dep_uri = loaded.uri().clone();
        if let Some(doc) = loader.parsed(&dep_uri) {
            return Ok((doc, dep_uri));
        }
    }
    if let Some(path) = clicked_uri.to_file_path()
        && let Some(dir) = path.parent()
    {
        for ext in ["eventb", "txt"] {
            let candidate = dir.join(format!("{name}.{ext}"));
            if let Some(candidate_uri) = Uri::from_file_path(&candidate)
                && let Some(doc) = loader.parsed(&candidate_uri)
                && doc.components().iter().any(|c| c.name() == name)
            {
                return Ok((doc, candidate_uri));
            }
        }
    }
    Err(AnimateError::MissingComponent(name.to_string()))
}

/// Record one parsed component into the closure and return its dependency
/// names (REFINES/SEES for machines, EXTENDS for contexts — read through the
/// shared edge SSOT, so the closure can never miss an edge kind the
/// cross-reference checks know about).
fn record_component(component: &rossi::Component, uri: &Uri, closure: &mut Closure) -> Vec<String> {
    let mut info = ComponentInfo {
        name: component.name().to_string(),
        uri: uri.clone(),
        event_names: HashSet::new(),
    };
    if let rossi::Component::Machine(machine) = component {
        let ascii = rossi::PrettyPrinter::ascii();
        for invariant in &machine.invariants {
            let Some(label) = invariant.label.clone() else {
                continue;
            };
            // The canonical rendering comes from the emitter's own
            // canonicaliser — the `.bcm` code the tool prints is produced
            // through that exact function, so the two cannot drift.
            let mut renderings = vec![
                normalize_predicate(&rossi_build::normalize::canonical_predicate(
                    &invariant.predicate,
                )),
                normalize_predicate(&ascii.print_formula_predicate(&invariant.predicate)),
            ];
            renderings.dedup();
            closure.invariants.push(InvariantInfo {
                label,
                component: machine.name.clone(),
                uri: uri.clone(),
                renderings,
            });
        }
        if machine.initialisation.is_some() {
            info.event_names.insert("INITIALISATION".to_string());
        }
        info.event_names
            .extend(machine.events.iter().map(|e| e.name.clone()));
    }
    closure.infos.push(info);
    closure.components.push(rossi::NamedComponent {
        filename: rossi::component_filename(component),
        component: component.clone(),
    });
    crate::diagnostics::component_references(component)
        .into_iter()
        .map(|(_, name)| name.to_string())
        .collect()
}

/// Whitespace-insensitive form used to compare predicate renderings with the
/// tool's printed forms.
pub(crate) fn normalize_predicate(predicate: &str) -> String {
    predicate.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Statically check the closure in memory, producing the `.bcc`/`.bcm`
/// checked files and the generated `.bpo`/`.bps` proof-obligation files.
fn build_in_memory(closure: &Closure) -> Result<rossi_build::BuildResult, AnimateError> {
    let mut project_components = Vec::with_capacity(closure.components.len());
    for named in &closure.components {
        let xml = rossi::to_xml(&named.component);
        let component = rossi_build::ProjectComponent::from_xml(&named.filename, &xml)
            .map_err(|e| AnimateError::Io(e.to_string()))?;
        project_components.push(component);
    }
    let project = rossi_build::Project::new(PROJECT_NAME, project_components);
    let result = rossi_build::build(&project);
    let findings = super::diagnostics::build_findings(
        result
            .diagnostics
            .iter()
            .filter(|d| d.severity == rossi_build::Severity::Error),
        closure,
    );
    if !findings.is_empty() {
        return Err(AnimateError::BuildFailed(findings));
    }
    Ok(result)
}

/// The number of proof-obligation sequents in the clicked machine's
/// generated `.bpo` file — the run is pinned to that machine with `-m`, so
/// ancestor and context obligations don't inflate the watchdog deadline. On
/// a fresh build every sequent is open. Counted by start-tag scan rather than
/// a full parse: the emitter always writes `name` as the first attribute, so
/// the space-terminated prefix is guaranteed (see
/// `rossi_build::pog::reconcile`, which relies on the same invariant).
pub(crate) fn count_po_sequents(build: &rossi_build::BuildResult, machine: &str) -> usize {
    build.file(&format!("{machine}.bpo")).map_or(0, |file| {
        file.contents.matches("<org.eventb.core.poSequent ").count()
    })
}

/// Write a complete Rodin project (sources + checked + proof files) to a
/// fresh temp directory. The directory is removed when the guard drops.
fn write_temp_project(
    components: &[rossi::NamedComponent],
    build_result: &rossi_build::BuildResult,
) -> Result<tempfile::TempDir, AnimateError> {
    let dir = tempfile::Builder::new()
        .prefix("rossi-animate-")
        .tempdir()
        .map_err(|e| AnimateError::Io(e.to_string()))?;
    rossi::write_project_directory(dir.path(), components, PROJECT_NAME)
        .map_err(|e| AnimateError::Io(e.to_string()))?;
    for file in &build_result.files {
        // The guard every consumer of `ScFile::filename` applies before
        // touching the filesystem (see `rossi_build::is_normal_path_component`).
        if !rossi_build::is_normal_path_component(&file.filename) {
            return Err(AnimateError::Io(format!(
                "unsafe generated filename {:?}",
                file.filename
            )));
        }
        std::fs::write(dir.path().join(&file.filename), &file.contents)
            .map_err(|e| AnimateError::Io(e.to_string()))?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn open(documents: &DocumentManager, xref: &CrossReferenceManager, uri: &str, text: &str) {
        let url = uri.parse::<Uri>().unwrap();
        documents.open(url.clone(), 1, text.to_string());
        xref.update_component(url.as_str().to_owned(), text);
    }

    #[test]
    fn closure_follows_refines_sees_extends_and_prefers_buffers() {
        let documents = DocumentManager::new();
        let xref = CrossReferenceManager::new();
        open(&documents, &xref, "file:///c0.eventb", "CONTEXT c0\nEND\n");
        open(
            &documents,
            &xref,
            "file:///c1.eventb",
            "CONTEXT c1\nEXTENDS c0\nEND\n",
        );
        open(
            &documents,
            &xref,
            "file:///m0.eventb",
            "MACHINE m0\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n",
        );
        open(
            &documents,
            &xref,
            "file:///m1.eventb",
            "MACHINE m1\nREFINES m0\nSEES c1\nEND\n",
        );

        let uri = ("file:///m1.eventb").parse::<Uri>().unwrap();
        let closure = collect_closure(&xref, &documents, &uri, "m1").unwrap();
        let mut names: Vec<&str> = closure
            .components
            .iter()
            .map(|c| c.component.name())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["c0", "c1", "m0", "m1"]);
        // m0's labeled invariant is recorded with its declaring machine.
        assert_eq!(closure.invariants.len(), 1);
        assert_eq!(closure.invariants[0].label, "inv1");
        assert_eq!(closure.invariants[0].component, "m0");
        // Structural info covers INITIALISATION for PO anchoring.
        let m0 = closure.infos.iter().find(|i| i.name == "m0").unwrap();
        assert!(m0.event_names.contains("INITIALISATION"));
        assert!(
            closure
                .invariants
                .iter()
                .any(|inv| inv.component == "m0" && inv.label == "inv1")
        );
    }

    #[test]
    fn closure_falls_back_to_sibling_files_on_disk() {
        let tmp = TempDir::new("animate-closure");
        std::fs::write(tmp.join("c0.eventb"), "CONTEXT c0\nEND\n").unwrap();
        let machine_path = tmp.join("m0.eventb");
        std::fs::write(&machine_path, "MACHINE m0\nSEES c0\nEND\n").unwrap();

        // Nothing open and nothing indexed: only the sibling fallback can
        // resolve `c0`.
        let documents = DocumentManager::new();
        let xref = CrossReferenceManager::new();
        let uri = Uri::from_file_path(&machine_path).unwrap();
        let closure = collect_closure(&xref, &documents, &uri, "m0").unwrap();
        assert_eq!(closure.components.len(), 2);
    }

    #[test]
    fn dirty_parse_aborts_the_closure() {
        let documents = DocumentManager::new();
        let xref = CrossReferenceManager::new();
        open(
            &documents,
            &xref,
            "file:///m.eventb",
            "MACHINE m\nVARIABLES\n    x y (\nEND\n",
        );
        let uri = ("file:///m.eventb").parse::<Uri>().unwrap();
        let error = collect_closure(&xref, &documents, &uri, "m").unwrap_err();
        assert_eq!(error, AnimateError::ParseFailed("m".to_string()));
    }

    #[test]
    fn contexts_are_rejected_and_missing_machines_reported() {
        let documents = DocumentManager::new();
        let xref = CrossReferenceManager::new();
        open(&documents, &xref, "file:///c.eventb", "CONTEXT c\nEND\n");
        let uri = ("file:///c.eventb").parse::<Uri>().unwrap();
        assert_eq!(
            collect_closure(&xref, &documents, &uri, "c").unwrap_err(),
            AnimateError::NotAMachine("c".to_string())
        );
        assert_eq!(
            collect_closure(&xref, &documents, &uri, "ghost").unwrap_err(),
            AnimateError::MissingComponent("ghost".to_string())
        );
    }

    #[test]
    fn prepare_builds_and_writes_a_complete_temp_project() {
        let documents = DocumentManager::new();
        let xref = CrossReferenceManager::new();
        open(&documents, &xref, "file:///m.eventb", PROVABLE_MACHINE);
        let uri = ("file:///m.eventb").parse::<Uri>().unwrap();
        let prepared = prepare(&xref, &documents, &uri, "m", AnimateMode::Check, None).unwrap();
        let dir = prepared.temp_dir.path();
        assert!(dir.join("m.bum").is_file());
        assert!(dir.join("m.bcm").is_file());
        assert!(dir.join("m.bpo").is_file());
        assert!(dir.join("m.bps").is_file());
        assert!(dir.join(".project").is_file());
        // INITIALISATION x :| … generates at least the inv1 INV obligation.
        assert!(prepared.po_count >= 1, "po_count = {}", prepared.po_count);
        let path = dir.to_path_buf();
        drop(prepared);
        assert!(!path.exists(), "the temp project is removed on drop");
    }

    #[test]
    fn static_errors_abort_with_anchored_findings() {
        let documents = DocumentManager::new();
        let xref = CrossReferenceManager::new();
        // `y` is never declared: the static check reports an error.
        open(
            &documents,
            &xref,
            "file:///m.eventb",
            "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 y ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n",
        );
        let uri = ("file:///m.eventb").parse::<Uri>().unwrap();
        match prepare(&xref, &documents, &uri, "m", AnimateMode::Check, None).unwrap_err() {
            AnimateError::BuildFailed(findings) => {
                assert!(!findings.is_empty());
                for finding in &findings {
                    assert_eq!(finding.code, "animate-build");
                    assert_eq!(finding.uri, uri);
                    assert_eq!(finding.component, "m");
                }
                // Pins the `Component.label` origin shape empirically.
                let inv = findings
                    .iter()
                    .find(|f| f.message.contains("m.inv1"))
                    .unwrap_or_else(|| panic!("an inv1 error among {findings:?}"));
                assert_eq!(
                    inv.anchor,
                    super::super::diagnostics::Anchor::InvariantLabel("inv1".to_string())
                );
                assert!(
                    inv.message.contains("unknown identifier"),
                    "{}",
                    inv.message
                );
            }
            other => panic!("expected BuildFailed, got {other:?}"),
        }
    }

    const PROVABLE_MACHINE: &str = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n";

    /// Copy a baseline temp project's generated `m.bpo`/`m.bps` into `to`,
    /// doctoring every status to discharged — a recorded proof state
    /// claiming everything is proven.
    fn write_discharged(from: &Path, to: &Path) {
        let bpo = std::fs::read_to_string(from.join("m.bpo")).unwrap();
        let bps = std::fs::read_to_string(from.join("m.bps")).unwrap();
        std::fs::write(to.join("m.bpo"), bpo).unwrap();
        std::fs::write(
            to.join("m.bps"),
            bps.replace("confidence=\"-99\"", "confidence=\"1000\""),
        )
        .unwrap();
    }

    /// A workspace holding `m` open in a buffer, plus a "recorded" proof
    /// state: the generated `m.bpo`/`m.bps` pair with every confidence
    /// doctored to 1000 (discharged), written into a directory posing as
    /// the shared Rodin workspace project.
    fn discharged_fixture() -> (DocumentManager, CrossReferenceManager, Uri, TempDir, usize) {
        let documents = DocumentManager::new();
        let xref = CrossReferenceManager::new();
        open(&documents, &xref, "file:///m.eventb", PROVABLE_MACHINE);
        let uri = ("file:///m.eventb").parse::<Uri>().unwrap();

        let baseline = prepare(&xref, &documents, &uri, "m", AnimateMode::Po, None).unwrap();
        assert!(baseline.po_count >= 1, "the fixture machine must have POs");
        let recorded = TempDir::new("animate-proof-state");
        write_discharged(baseline.temp_dir.path(), recorded.path());
        (documents, xref, uri, recorded, baseline.po_count)
    }

    #[test]
    fn po_mode_carries_matching_proof_state_and_counts_open() {
        let (documents, xref, uri, recorded, _) = discharged_fixture();
        let prepared = prepare(
            &xref,
            &documents,
            &uri,
            "m",
            AnimateMode::Po,
            Some(recorded.path()),
        )
        .unwrap();
        let bps = std::fs::read_to_string(prepared.temp_dir.path().join("m.bps")).unwrap();
        assert!(
            bps.contains("confidence=\"1000\""),
            "recorded discharges carry into the temp project: {bps}"
        );
        assert_eq!(prepared.po_count, 0, "every obligation is discharged");
    }

    #[test]
    fn po_mode_resets_stale_proof_state_after_edits() {
        let (documents, xref, uri, recorded, baseline_count) = discharged_fixture();
        // The initialisation changes, so the INV sequent's goal changes
        // (`0 ∈ ℕ` → `1 ∈ ℕ`): the recorded discharge is for a different
        // model and must not carry.
        documents.change(
            &uri,
            2,
            vec![crate::lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: PROVABLE_MACHINE.replace("x := 0", "x := 1"),
            }],
        );
        let prepared = prepare(
            &xref,
            &documents,
            &uri,
            "m",
            AnimateMode::Po,
            Some(recorded.path()),
        )
        .unwrap();
        let bps = std::fs::read_to_string(prepared.temp_dir.path().join("m.bps")).unwrap();
        assert!(
            !bps.contains("confidence=\"1000\""),
            "stale discharges must reset to unattempted: {bps}"
        );
        assert_eq!(prepared.po_count, baseline_count);
    }

    #[test]
    fn po_mode_reads_proof_state_next_to_sources() {
        // An on-disk model whose recorded proof files sit next to the
        // sources (the proof mirror's checkout copies): the fallback path.
        let tmp = TempDir::new("animate-checkout-proofs");
        let machine_path = tmp.join("m.eventb");
        std::fs::write(&machine_path, PROVABLE_MACHINE).unwrap();
        let documents = DocumentManager::new();
        let xref = CrossReferenceManager::new();
        let uri = Uri::from_file_path(&machine_path).unwrap();

        let baseline = prepare(&xref, &documents, &uri, "m", AnimateMode::Po, None).unwrap();
        write_discharged(baseline.temp_dir.path(), tmp.path());

        let prepared = prepare(&xref, &documents, &uri, "m", AnimateMode::Po, None).unwrap();
        assert_eq!(prepared.po_count, 0, "checkout proof state is honored");
    }

    #[test]
    fn check_mode_ignores_recorded_proof_state() {
        let (documents, xref, uri, recorded, baseline_count) = discharged_fixture();
        let prepared = prepare(
            &xref,
            &documents,
            &uri,
            "m",
            AnimateMode::Check,
            Some(recorded.path()),
        )
        .unwrap();
        let bps = std::fs::read_to_string(prepared.temp_dir.path().join("m.bps")).unwrap();
        assert!(
            !bps.contains("confidence=\"1000\""),
            "Check mode keeps pristine statuses: {bps}"
        );
        assert_eq!(prepared.po_count, baseline_count);
    }
}
