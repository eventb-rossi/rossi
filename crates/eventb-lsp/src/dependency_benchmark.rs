//! Release-mode benchmark harness for dependency-aware LSP requests.

#[path = "../benchmark_support/mod.rs"]
mod support;

use std::hint::black_box;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::benchmark_metrics::{self, BenchmarkMetrics};
use crate::completion::{self, CompletionProvider};
use crate::component_loader::ComponentLoader;
use crate::config::{CompletionConfig, FormatConfig};
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::hover::{self, HoverProvider};
use crate::lsp_types::Position;
use crate::references::ReferenceProvider;
use crate::rename::RenameProvider;
use crate::resolved_environment::ResolvedEnvironment;
use crate::symbols::{
    SymbolIdentity, candidate_components_for_symbol, resolve_symbol_identity_in_component,
};

struct IndexedWorkspace {
    manager: Arc<CrossReferenceManager>,
    documents: Arc<DocumentManager>,
}

#[derive(Debug)]
struct CaseResult {
    model: String,
    operation: &'static str,
    cache_state: &'static str,
    samples: usize,
    median_ns: u128,
    p95_ns: u128,
    result_items: usize,
    metrics: BenchmarkMetrics,
}

#[test]
#[ignore = "requires EVENTB_CORPUS_DIR and release mode"]
fn dependency_environment_baseline() {
    if cfg!(debug_assertions) {
        panic!("dependency benchmarks must run with cargo test --release");
    }
    let fixtures = support::prepare_models("wall").unwrap_or_else(|error| panic!("{error}"));
    let (warmups, samples) = support::benchmark_counts();
    let mut results = Vec::new();

    for fixture in &fixtures {
        benchmark_model(fixture, warmups, samples, &mut results);
    }

    let report = support::target_root().join("lsp-dependency-environment-wall.tsv");
    write_report(&report, &results).expect("write wall-time benchmark report");
    println!("wall-time report: {}", report.display());
}

fn benchmark_model(
    fixture: &support::ModelFixture,
    warmups: usize,
    samples: usize,
    results: &mut Vec<CaseResult>,
) {
    let root = fixture.component(&fixture.spec.root).component.clone();
    let manager = index_manager(fixture);

    benchmark_cache_states(
        &fixture.spec.slug,
        "resolved_environment",
        &manager,
        warmups,
        samples,
        results,
        |loader| ResolvedEnvironment::new(&root, loader).benchmark_cardinality(),
    );

    let warm_loader = ComponentLoader::new(&manager, None);
    let direct_environment = ResolvedEnvironment::new(&root, &warm_loader);
    results.push(measure_case(
        &fixture.spec.slug,
        "direct_edge_enumeration",
        "request_cache_warm",
        warmups,
        samples,
        || direct_environment.benchmark_direct_edges(),
    ));

    benchmark_cache_states(
        &fixture.spec.slug,
        "hover_environment",
        &manager,
        warmups,
        samples,
        results,
        |loader| hover::benchmark_environment_construction(&root, loader),
    );

    benchmark_cache_states(
        &fixture.spec.slug,
        "completion_environment",
        &manager,
        warmups,
        samples,
        results,
        |loader| completion::benchmark_environment_construction(&root, loader),
    );

    benchmark_complete_requests(fixture, &manager, warmups, samples, results);
    benchmark_warm_candidate_resolution(fixture, &manager, warmups, samples, results);
}

fn benchmark_cache_states(
    model: &str,
    operation: &'static str,
    manager: &CrossReferenceManager,
    warmups: usize,
    samples: usize,
    results: &mut Vec<CaseResult>,
    operation_fn: impl Fn(&ComponentLoader<'_>) -> usize,
) {
    results.push(measure_case(
        model,
        operation,
        "cold",
        warmups,
        samples,
        || {
            let loader = ComponentLoader::new(manager, None);
            operation_fn(&loader)
        },
    ));

    let warm_loader = ComponentLoader::new(manager, None);
    black_box(operation_fn(&warm_loader));
    results.push(measure_case(
        model,
        operation,
        "request_cache_warm",
        warmups,
        samples,
        || operation_fn(&warm_loader),
    ));
}

fn benchmark_complete_requests(
    fixture: &support::ModelFixture,
    manager: &Arc<CrossReferenceManager>,
    warmups: usize,
    samples: usize,
    results: &mut Vec<CaseResult>,
) {
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
    let root_workspace = workspace_with_open(fixture, Arc::clone(manager), &fixture.spec.root);

    let mut hover_provider = HoverProvider::new();
    hover_provider.set_cross_reference_manager(Arc::clone(&root_workspace.manager));
    hover_provider.set_document_manager(Arc::clone(&root_workspace.documents));
    let hover_params = support::hover_params(root_uri.clone(), root_position);
    results.push(measure_case(
        &fixture.spec.slug,
        "hover_request",
        "cold",
        warmups,
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
    results.push(measure_case(
        &fixture.spec.slug,
        "completion_request",
        "cold",
        warmups,
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
        workspace_with_open(fixture, Arc::clone(manager), &fixture.spec.reference_owner);
    let mut reference_provider = ReferenceProvider::new();
    reference_provider.set_cross_reference_manager(Arc::clone(&reference_workspace.manager));
    reference_provider.set_document_manager(Arc::clone(&reference_workspace.documents));
    let reference_params = support::reference_params(reference_uri, reference_position);
    results.push(measure_case(
        &fixture.spec.slug,
        "references_request",
        "cold_to_warm",
        warmups,
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
    let component_workspace =
        workspace_with_open(fixture, Arc::clone(manager), &fixture.spec.rename_component);

    let mut component_reference_provider = ReferenceProvider::new();
    component_reference_provider
        .set_cross_reference_manager(Arc::clone(&component_workspace.manager));
    component_reference_provider.set_document_manager(Arc::clone(&component_workspace.documents));
    let component_reference_params = support::reference_params(rename_uri.clone(), rename_position);
    results.push(measure_case(
        &fixture.spec.slug,
        "component_references_request",
        "not_applicable",
        warmups,
        samples,
        || {
            component_reference_provider
                .find_references(&component_reference_params, &rename_source.text)
                .map_or(0, |locations| locations.len())
        },
    ));

    let mut rename_provider = RenameProvider::new();
    rename_provider.set_cross_reference_manager(Arc::clone(&component_workspace.manager));
    rename_provider.set_document_manager(Arc::clone(&component_workspace.documents));
    let rename_params = support::rename_params(rename_uri, rename_position);
    results.push(measure_case(
        &fixture.spec.slug,
        "rename_request",
        "not_applicable",
        warmups,
        samples,
        || {
            rename_provider
                .rename(&rename_params, &rename_source.text)
                .and_then(|edit| edit.changes)
                .map_or(0, |changes| changes.values().map(Vec::len).sum())
        },
    ));
}

fn benchmark_warm_candidate_resolution(
    fixture: &support::ModelFixture,
    manager: &Arc<CrossReferenceManager>,
    warmups: usize,
    samples: usize,
    results: &mut Vec<CaseResult>,
) {
    let owner = fixture
        .component(&fixture.spec.reference_owner)
        .component
        .clone();
    let loader = ComponentLoader::new(manager, None);
    let symbol =
        resolve_symbol_identity_in_component(&owner, &fixture.spec.reference_symbol, &loader)
            .expect("reference symbol resolves in its owner");
    let candidates = candidate_components_for_symbol(&symbol, manager);
    black_box(resolve_candidates(&candidates, &symbol, &loader));

    results.push(measure_case(
        &fixture.spec.slug,
        "references_candidate_resolution",
        "request_cache_warm",
        warmups,
        samples,
        || resolve_candidates(&candidates, &symbol, &loader),
    ));
}

fn resolve_candidates(
    candidates: &[String],
    symbol: &SymbolIdentity,
    loader: &ComponentLoader<'_>,
) -> usize {
    candidates
        .iter()
        .filter_map(|name| loader.load(name))
        .filter(|loaded| {
            resolve_symbol_identity_in_component(loaded.component(), &symbol.name, loader)
                == Some(symbol.clone())
        })
        .count()
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

fn measure_case(
    model: &str,
    operation: &'static str,
    cache_state: &'static str,
    warmups: usize,
    samples: usize,
    mut operation_fn: impl FnMut() -> usize,
) -> CaseResult {
    for _ in 0..warmups {
        black_box(operation_fn());
    }

    let (result_items, metrics) = measure_metrics(&mut operation_fn);
    let verification = measure_metrics(&mut operation_fn);
    assert_eq!(
        (result_items, metrics),
        verification,
        "{model}/{operation}/{cache_state} produced unstable counters"
    );

    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let items = black_box(operation_fn());
        let elapsed = started.elapsed();
        assert!(
            items > 0,
            "{model}/{operation}/{cache_state} returned no results"
        );
        assert_eq!(
            result_items, items,
            "{model}/{operation}/{cache_state} changed result cardinality"
        );
        durations.push(elapsed);
    }
    durations.sort_unstable();
    let result = CaseResult {
        model: model.to_string(),
        operation,
        cache_state,
        samples,
        median_ns: percentile(&durations, 50).as_nanos(),
        p95_ns: percentile(&durations, 95).as_nanos(),
        result_items,
        metrics,
    };
    println!(
        "{model:14} {operation:34} {cache_state:20} p50={:>10.3} ms p95={:>10.3} ms",
        result.median_ns as f64 / 1_000_000.0,
        result.p95_ns as f64 / 1_000_000.0
    );
    result
}

fn measure_metrics(operation_fn: &mut impl FnMut() -> usize) -> (usize, BenchmarkMetrics) {
    benchmark_metrics::start();
    let items = black_box(operation_fn());
    let metrics = benchmark_metrics::stop();
    (items, metrics)
}

fn percentile(sorted: &[Duration], percentile: usize) -> Duration {
    let index = (sorted.len() * percentile).div_ceil(100).saturating_sub(1);
    sorted[index]
}

fn write_report(path: &std::path::Path, results: &[CaseResult]) -> std::io::Result<()> {
    let mut report = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(
        report,
        "model\toperation\tcache_state\tsamples\tmedian_ns\tp95_ns\tresult_items\t\
         environments\tqueue_pops\tunique_nodes\tloaded_nodes\tindexed_fallback_nodes\t\
         unavailable_nodes\tdirect_edge_queries\tdirect_edges\tloader_cache_hits\t\
         loader_cache_misses\tdocument_parse_reuses\tdisk_parses\t\
         component_candidate_uris\tcomponent_source_bytes\tcomponent_occurrence_scans\t\
         component_occurrences"
    )?;
    for result in results {
        let metrics = result.metrics;
        writeln!(
            report,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            result.model,
            result.operation,
            result.cache_state,
            result.samples,
            result.median_ns,
            result.p95_ns,
            result.result_items,
            metrics.environments,
            metrics.queue_pops,
            metrics.unique_nodes,
            metrics.loaded_nodes,
            metrics.indexed_fallback_nodes,
            metrics.unavailable_nodes,
            metrics.direct_edge_queries,
            metrics.direct_edges,
            metrics.loader_cache_hits,
            metrics.loader_cache_misses,
            metrics.document_parse_reuses,
            metrics.disk_parses,
            metrics.component_candidate_uris,
            metrics.component_source_bytes,
            metrics.component_occurrence_scans,
            metrics.component_occurrences,
        )?;
    }
    report.flush()
}

fn offset_position(text: &str, offset: usize) -> Position {
    crate::position::offset_to_position(text, offset)
}
