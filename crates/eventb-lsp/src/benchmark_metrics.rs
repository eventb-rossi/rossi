//! Test-only counters for the dependency-environment benchmark.

use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BenchmarkMetrics {
    pub(crate) environments: u64,
    pub(crate) queue_pops: u64,
    pub(crate) unique_nodes: u64,
    pub(crate) loaded_nodes: u64,
    pub(crate) indexed_fallback_nodes: u64,
    pub(crate) unavailable_nodes: u64,
    pub(crate) direct_edge_queries: u64,
    pub(crate) direct_edges: u64,
    pub(crate) loader_cache_hits: u64,
    pub(crate) loader_cache_misses: u64,
    pub(crate) document_parse_reuses: u64,
    pub(crate) disk_parses: u64,
    pub(crate) component_candidate_uris: u64,
    pub(crate) component_source_bytes: u64,
    pub(crate) component_occurrence_scans: u64,
    pub(crate) component_occurrences: u64,
}

thread_local! {
    static METRICS: Cell<Option<BenchmarkMetrics>> = const { Cell::new(None) };
}

pub(crate) fn start() {
    METRICS.with(|metrics| metrics.set(Some(BenchmarkMetrics::default())));
}

pub(crate) fn stop() -> BenchmarkMetrics {
    METRICS.with(Cell::take).unwrap_or_default()
}

fn update(f: impl FnOnce(&mut BenchmarkMetrics)) {
    METRICS.with(|metrics| {
        let Some(mut current) = metrics.get() else {
            return;
        };
        f(&mut current);
        metrics.set(Some(current));
    });
}

pub(crate) fn environment_started() {
    update(|metrics| metrics.environments += 1);
}

pub(crate) fn queue_popped() {
    update(|metrics| metrics.queue_pops += 1);
}

pub(crate) fn unique_node() {
    update(|metrics| metrics.unique_nodes += 1);
}

pub(crate) fn loaded_node() {
    update(|metrics| metrics.loaded_nodes += 1);
}

pub(crate) fn indexed_fallback_node() {
    update(|metrics| metrics.indexed_fallback_nodes += 1);
}

pub(crate) fn unavailable_node() {
    update(|metrics| metrics.unavailable_nodes += 1);
}

pub(crate) fn direct_edges(count: usize) {
    update(|metrics| {
        metrics.direct_edge_queries += 1;
        metrics.direct_edges += count as u64;
    });
}

pub(crate) fn loader_cache_hit() {
    update(|metrics| metrics.loader_cache_hits += 1);
}

pub(crate) fn loader_cache_miss() {
    update(|metrics| metrics.loader_cache_misses += 1);
}

pub(crate) fn document_parse_reuse() {
    update(|metrics| metrics.document_parse_reuses += 1);
}

pub(crate) fn disk_parse() {
    update(|metrics| metrics.disk_parses += 1);
}

pub(crate) fn component_occurrence_query(
    candidate_uris: usize,
    source_bytes: u64,
    occurrence_scans: usize,
    occurrences: usize,
) {
    update(|metrics| {
        metrics.component_candidate_uris += candidate_uris as u64;
        metrics.component_source_bytes += source_bytes;
        metrics.component_occurrence_scans += occurrence_scans as u64;
        metrics.component_occurrences += occurrences as u64;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occurrence_metrics_accept_byte_totals_beyond_u32() {
        let source_bytes = u64::from(u32::MAX) + 1;

        start();
        component_occurrence_query(0, source_bytes, 0, 0);
        let metrics = stop();

        assert_eq!(metrics.component_source_bytes, source_bytes);
    }
}
