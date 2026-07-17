//! Allocation measurements for dependency-aware LSP requests.

#[path = "../benchmark_support/mod.rs"]
mod support;

use std::alloc::System;
use std::hint::black_box;
use std::io::Write;
use std::sync::Arc;

use eventb_lsp::completion::CompletionProvider;
use eventb_lsp::config::{CompletionConfig, FormatConfig};
use eventb_lsp::cross_references::CrossReferenceManager;
use eventb_lsp::document::DocumentManager;
use eventb_lsp::hover::HoverProvider;
use eventb_lsp::lsp_types::Position;
use eventb_lsp::references::ReferenceProvider;
use eventb_lsp::rename::RenameProvider;
use rossi::deps::DependencyGraph;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[derive(Debug)]
struct AllocationResult {
    model: String,
    operation: &'static str,
    samples: usize,
    allocations: usize,
    reallocations: usize,
    bytes_allocated: usize,
    bytes_reallocated: isize,
    result_items: usize,
}

struct IndexedWorkspace {
    manager: Arc<CrossReferenceManager>,
    documents: Arc<DocumentManager>,
}

#[test]
#[ignore = "requires EVENTB_CORPUS_DIR and release mode"]
fn dependency_environment_allocations() {
    if cfg!(debug_assertions) {
        panic!("allocation benchmarks must run with cargo test --release");
    }
    let fixtures = support::prepare_models("heap").unwrap_or_else(|error| panic!("{error}"));
    let samples = support::allocation_samples();
    let mut results = Vec::new();
    for fixture in &fixtures {
        benchmark_model(fixture, samples, &mut results);
    }

    let report = support::target_root().join("lsp-dependency-environment-allocations.tsv");
    write_report(&report, &results).expect("write allocation benchmark report");
    println!("allocation report: {}", report.display());
}

fn benchmark_model(
    fixture: &support::ModelFixture,
    samples: usize,
    results: &mut Vec<AllocationResult>,
) {
    {
        let graph = DependencyGraph::from_components(
            fixture
                .components
                .values()
                .map(|component| &component.component),
        );
        let mut reachable: Vec<_> = graph
            .all_reachable(&fixture.spec.root)
            .into_iter()
            .collect();
        reachable.push(fixture.spec.root.to_string());
        reachable.sort();
        reachable.dedup();
        results.push(measure_allocations(
            &fixture.spec.slug,
            "direct_edge_enumeration",
            samples,
            || {
                reachable
                    .iter()
                    .filter_map(|name| {
                        let kind = graph.kind_of(name)?;
                        let dependencies = graph
                            .references_of_kind(kind, name)
                            .into_iter()
                            .flatten()
                            .flat_map(|(edge, names)| {
                                names
                                    .into_iter()
                                    .map(move |name| (edge.target_kind(), name))
                            })
                            .collect::<Vec<_>>();
                        Some(dependencies.len())
                    })
                    .sum()
            },
        ));
    }

    let manager = index_manager(fixture);

    let root_source = fixture.component(&fixture.spec.root);
    let root_uri = support::file_uri(&root_source.path);
    let root_position = offset_position(
        &root_source.text,
        fixture.declaration_offset(
            &fixture.spec.root,
            &fixture.spec.hover_section,
            &fixture.spec.hover_symbol,
        ),
    );
    let root_workspace = workspace_with_open(fixture, Arc::clone(&manager), &fixture.spec.root);

    let mut hover_provider = HoverProvider::new();
    hover_provider.set_cross_reference_manager(Arc::clone(&root_workspace.manager));
    hover_provider.set_document_manager(Arc::clone(&root_workspace.documents));
    let hover_params = support::hover_params(root_uri.clone(), root_position);
    results.push(measure_allocations(
        &fixture.spec.slug,
        "hover_request",
        samples,
        || {
            hover_provider
                .hover(&hover_params, &root_source.text)
                .map_or(0, |_| 1)
        },
    ));

    let mut completion_provider = CompletionProvider::new();
    completion_provider.set_cross_reference_manager(Arc::clone(&root_workspace.manager));
    completion_provider.set_document_manager(Arc::clone(&root_workspace.documents));
    let completion_position = Position {
        character: root_position.character + fixture.spec.hover_symbol.len() as u32,
        ..root_position
    };
    let completion_params = support::completion_params(root_uri, completion_position);
    let completion_config = CompletionConfig::default();
    let format_config = FormatConfig::default();
    results.push(measure_allocations(
        &fixture.spec.slug,
        "completion_request",
        samples,
        || {
            completion_provider
                .complete(
                    &completion_params,
                    &root_source.text,
                    &completion_config,
                    &format_config,
                )
                .map_or(0, support::completion_len)
        },
    ));

    let reference_source = fixture.component(&fixture.spec.reference_owner);
    let reference_uri = support::file_uri(&reference_source.path);
    let reference_position = offset_position(
        &reference_source.text,
        fixture.declaration_offset(
            &fixture.spec.reference_owner,
            &fixture.spec.reference_section,
            &fixture.spec.reference_symbol,
        ),
    );
    let reference_workspace =
        workspace_with_open(fixture, Arc::clone(&manager), &fixture.spec.reference_owner);
    let mut reference_provider = ReferenceProvider::new();
    reference_provider.set_cross_reference_manager(Arc::clone(&reference_workspace.manager));
    reference_provider.set_document_manager(Arc::clone(&reference_workspace.documents));
    let reference_params = support::reference_params(reference_uri, reference_position);
    results.push(measure_allocations(
        &fixture.spec.slug,
        "references_request",
        samples,
        || {
            reference_provider
                .find_references(&reference_params, &reference_source.text)
                .map_or(0, |locations| locations.len())
        },
    ));

    let rename_source = fixture.component(&fixture.spec.rename_component);
    let rename_uri = support::file_uri(&rename_source.path);
    let rename_position = offset_position(
        &rename_source.text,
        fixture.component_name_offset(&fixture.spec.rename_component),
    );
    let rename_workspace = workspace_with_open(
        fixture,
        Arc::clone(&manager),
        &fixture.spec.rename_component,
    );
    let mut rename_provider = RenameProvider::new();
    rename_provider.set_cross_reference_manager(Arc::clone(&rename_workspace.manager));
    rename_provider.set_document_manager(Arc::clone(&rename_workspace.documents));
    let rename_params = support::rename_params(rename_uri, rename_position);
    results.push(measure_allocations(
        &fixture.spec.slug,
        "rename_request",
        samples,
        || {
            rename_provider
                .rename(&rename_params, &rename_source.text)
                .and_then(|edit| edit.changes)
                .map_or(0, |changes| changes.values().map(Vec::len).sum())
        },
    ));
}

fn measure_allocations(
    model: &str,
    operation: &'static str,
    samples: usize,
    mut operation_fn: impl FnMut() -> usize,
) -> AllocationResult {
    for _ in 0..2 {
        black_box(operation_fn());
    }

    let mut allocations = Vec::with_capacity(samples);
    let mut reallocations = Vec::with_capacity(samples);
    let mut bytes_allocated = Vec::with_capacity(samples);
    let mut bytes_reallocated = Vec::with_capacity(samples);
    let mut expected_items = None;
    for _ in 0..samples {
        let region = Region::new(GLOBAL);
        let items = black_box(operation_fn());
        let stats = region.change();
        assert!(items > 0, "{model}/{operation} returned no results");
        assert_eq!(
            *expected_items.get_or_insert(items),
            items,
            "{model}/{operation} changed result cardinality"
        );
        allocations.push(stats.allocations);
        reallocations.push(stats.reallocations);
        bytes_allocated.push(stats.bytes_allocated);
        bytes_reallocated.push(stats.bytes_reallocated);
    }
    allocations.sort_unstable();
    reallocations.sort_unstable();
    bytes_allocated.sort_unstable();
    bytes_reallocated.sort_unstable();
    let result = AllocationResult {
        model: model.to_string(),
        operation,
        samples,
        allocations: median(&allocations),
        reallocations: median(&reallocations),
        bytes_allocated: median(&bytes_allocated),
        bytes_reallocated: median(&bytes_reallocated),
        result_items: expected_items.expect("at least one sample"),
    };
    println!(
        "{model:14} {operation:28} allocs={:>10} bytes={:>12}",
        result.allocations, result.bytes_allocated
    );
    result
}

fn median<T: Copy>(values: &[T]) -> T {
    values[values.len() / 2]
}

fn write_report(path: &std::path::Path, results: &[AllocationResult]) -> std::io::Result<()> {
    let mut report = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(
        report,
        "model\toperation\tsamples\tallocations\treallocations\tbytes_allocated\t\
         bytes_reallocated\tresult_items"
    )?;
    for result in results {
        writeln!(
            report,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            result.model,
            result.operation,
            result.samples,
            result.allocations,
            result.reallocations,
            result.bytes_allocated,
            result.bytes_reallocated,
            result.result_items,
        )?;
    }
    report.flush()
}

fn index_manager(fixture: &support::ModelFixture) -> Arc<CrossReferenceManager> {
    let manager = Arc::new(CrossReferenceManager::new());
    for component in fixture.components.values() {
        let uri = support::file_uri(&component.path);
        manager.index_components(uri.to_string(), std::slice::from_ref(&component.component));
    }
    manager
}

fn workspace_with_open(
    fixture: &support::ModelFixture,
    manager: Arc<CrossReferenceManager>,
    name: &str,
) -> IndexedWorkspace {
    let documents = Arc::new(DocumentManager::new());
    let component = fixture.component(name);
    let uri = support::file_uri(&component.path);
    documents.open(uri.clone(), 1, component.text.clone());
    documents
        .parse_result(&uri)
        .unwrap_or_else(|| panic!("{}::{name} parses", fixture.spec.slug));
    IndexedWorkspace { manager, documents }
}

fn offset_position(text: &str, offset: usize) -> Position {
    eventb_lsp::position::offset_to_position(text, offset)
}
