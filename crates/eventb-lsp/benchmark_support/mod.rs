//! Shared fixture preparation for the dependency-environment benchmarks.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tower_lsp::lsp_types::{
    CompletionParams, CompletionResponse, HoverParams, PartialResultParams, Position,
    ReferenceContext, ReferenceParams, RenameParams, TextDocumentIdentifier,
    TextDocumentPositionParams, Url, WorkDoneProgressParams,
};

pub const DEFAULT_WARMUPS: usize = 5;
pub const DEFAULT_SAMPLES: usize = 20;
const CONFIG_FILE: &str = "rossi-lsp-dependency-benchmarks.json";

#[derive(Debug, Deserialize)]
pub struct ModelSpec {
    pub slug: String,
    pub archive: String,
    pub component_count: usize,
    pub root: String,
    pub hover_symbol: String,
    #[serde(default = "default_hover_section")]
    pub hover_section: String,
    pub reference_owner: String,
    pub reference_section: String,
    pub reference_symbol: String,
    pub rename_component: String,
}

#[derive(Debug)]
pub struct FixtureComponent {
    pub component: rossi::Component,
    pub text: String,
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct ModelFixture {
    pub spec: ModelSpec,
    pub components: BTreeMap<String, FixtureComponent>,
}

impl ModelFixture {
    pub fn component(&self, name: &str) -> &FixtureComponent {
        self.components
            .get(name)
            .unwrap_or_else(|| panic!("{} has no component {name}", self.spec.slug))
    }

    pub fn declaration_offset(&self, component: &str, section: &str, symbol: &str) -> usize {
        declaration_offset(&self.component(component).text, section, symbol).unwrap_or_else(|| {
            panic!(
                "{}: cannot find {component}::{symbol} in {section}",
                self.spec.slug
            )
        })
    }

    pub fn component_name_offset(&self, component: &str) -> usize {
        self.component(component)
            .component
            .name_span()
            .map(|span| span.start)
            .unwrap_or_else(|| panic!("{} has no declaration for {component}", self.spec.slug))
    }
}

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("eventb-lsp is under <workspace>/crates")
        .to_path_buf()
}

pub fn target_root() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                workspace_root().join(path)
            }
        }
        None => workspace_root().join("target"),
    }
}

pub fn corpus_root() -> Result<PathBuf, String> {
    let value = std::env::var_os("EVENTB_CORPUS_DIR")
        .ok_or_else(|| "EVENTB_CORPUS_DIR must be set to the benchmark corpus".to_string())?;
    let path = PathBuf::from(value);
    Ok(if path.is_absolute() {
        path
    } else {
        workspace_root().join(path)
    })
}

pub fn benchmark_counts() -> (usize, usize) {
    let warmups = env_count("ROSSI_LSP_BENCH_WARMUPS", DEFAULT_WARMUPS);
    let samples = env_count("ROSSI_LSP_BENCH_SAMPLES", DEFAULT_SAMPLES);
    (warmups, samples)
}

pub fn allocation_samples() -> usize {
    env_count("ROSSI_LSP_BENCH_ALLOC_SAMPLES", 5)
}

pub fn file_uri(path: &Path) -> Url {
    Url::from_file_path(path)
        .unwrap_or_else(|()| panic!("invalid fixture path: {}", path.display()))
}

pub fn hover_params(uri: Url, position: Position) -> HoverParams {
    HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

pub fn completion_params(uri: Url, position: Position) -> CompletionParams {
    CompletionParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: None,
    }
}

pub fn reference_params(uri: Url, position: Position) -> ReferenceParams {
    ReferenceParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration: true,
        },
    }
}

pub fn rename_params(uri: Url, position: Position) -> RenameParams {
    RenameParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        },
        new_name: "BenchRenamedComponent".to_string(),
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

pub fn completion_len(response: CompletionResponse) -> usize {
    match response {
        CompletionResponse::Array(items) => items.len(),
        CompletionResponse::List(list) => list.items.len(),
    }
}

pub fn prepare_models(fixture_namespace: &str) -> Result<Vec<ModelFixture>, String> {
    let corpus = corpus_root()?;
    if !corpus.is_dir() {
        return Err(format!(
            "Event-B corpus not found at {}; set EVENTB_CORPUS_DIR",
            corpus.display()
        ));
    }

    let fixture_root = target_root().join(format!(
        "lsp-dependency-environment-fixtures-{fixture_namespace}"
    ));
    std::fs::create_dir_all(&fixture_root)
        .map_err(|error| format!("create {}: {error}", fixture_root.display()))?;

    let config_path = corpus.join(CONFIG_FILE);
    let config = std::fs::read_to_string(&config_path)
        .map_err(|error| format!("read {}: {error}", config_path.display()))?;
    let specs: Vec<ModelSpec> = serde_json::from_str(&config)
        .map_err(|error| format!("parse {}: {error}", config_path.display()))?;
    validate_specs(&config_path, &specs)?;

    specs
        .into_iter()
        .map(|spec| prepare_model(&corpus, &fixture_root, spec))
        .collect()
}

fn prepare_model(
    corpus: &Path,
    fixture_root: &Path,
    spec: ModelSpec,
) -> Result<ModelFixture, String> {
    let archive_path = corpus.join(&spec.archive);
    let bytes = std::fs::read(&archive_path)
        .map_err(|error| format!("read {}: {error}", archive_path.display()))?;
    let mut parsed = rossi::parse_zip(&bytes)
        .map_err(|error| format!("parse {}: {error}", archive_path.display()))?;
    if parsed.len() != spec.component_count {
        return Err(format!(
            "{}: expected {} components, found {}",
            spec.slug,
            spec.component_count,
            parsed.len()
        ));
    }
    parsed.sort_by(|left, right| left.component.name().cmp(right.component.name()));

    let model_root = fixture_root.join(&spec.slug);
    if model_root.exists() {
        std::fs::remove_dir_all(&model_root)
            .map_err(|error| format!("clear {}: {error}", model_root.display()))?;
    }
    std::fs::create_dir_all(&model_root)
        .map_err(|error| format!("create {}: {error}", model_root.display()))?;

    let mut components = BTreeMap::new();
    let printer = rossi::PrettyPrinter::new();
    for (index, named) in parsed.into_iter().enumerate() {
        let name = named.component.name().to_string();
        if components.contains_key(&name) {
            return Err(format!("{}: duplicate component name {name}", spec.slug));
        }
        let text = printer.print_component(&named.component);
        let component = rossi::parse(&text).map_err(|error| {
            format!(
                "{}::{name}: printed text does not parse: {error}",
                spec.slug
            )
        })?;
        let safe_name: String = name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        let path = model_root.join(format!("{index:03}-{safe_name}.eventb"));
        std::fs::write(&path, &text)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        components.insert(
            name,
            FixtureComponent {
                component,
                text,
                path,
            },
        );
    }

    let fixture = ModelFixture { spec, components };
    fixture.component(&fixture.spec.root);
    fixture.declaration_offset(
        &fixture.spec.root,
        &fixture.spec.hover_section,
        &fixture.spec.hover_symbol,
    );
    fixture.declaration_offset(
        &fixture.spec.reference_owner,
        &fixture.spec.reference_section,
        &fixture.spec.reference_symbol,
    );
    fixture.component_name_offset(&fixture.spec.rename_component);
    Ok(fixture)
}

fn validate_specs(config_path: &Path, specs: &[ModelSpec]) -> Result<(), String> {
    if specs.is_empty() {
        return Err(format!("{} contains no model specs", config_path.display()));
    }
    let mut slugs = BTreeSet::new();
    for spec in specs {
        if spec.slug.is_empty()
            || !spec
                .slug
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            return Err(format!(
                "{} contains invalid slug {:?}",
                config_path.display(),
                spec.slug
            ));
        }
        if !slugs.insert(&spec.slug) {
            return Err(format!(
                "{} contains duplicate slug {}",
                config_path.display(),
                spec.slug
            ));
        }
    }
    Ok(())
}

fn default_hover_section() -> String {
    "VARIABLES".to_string()
}

fn declaration_offset(text: &str, section: &str, symbol: &str) -> Option<usize> {
    let mut in_section = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let without_newline = line.trim_end_matches(['\r', '\n']);
        if !without_newline.starts_with(char::is_whitespace) {
            in_section = without_newline.trim() == section;
        } else if in_section {
            let declaration = without_newline
                .trim()
                .split("//")
                .next()
                .unwrap_or_default()
                .trim();
            if declaration.split_whitespace().next() == Some(symbol) {
                return without_newline.find(symbol).map(|column| offset + column);
            }
        }
        offset += line.len();
    }
    None
}

fn env_count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("{name} must be a positive integer"))
        })
        .unwrap_or(default)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(slug: &str) -> ModelSpec {
        ModelSpec {
            slug: slug.to_string(),
            archive: "model.zip".to_string(),
            component_count: 1,
            root: "M".to_string(),
            hover_symbol: "x".to_string(),
            hover_section: "VARIABLES".to_string(),
            reference_owner: "M".to_string(),
            reference_section: "VARIABLES".to_string(),
            reference_symbol: "x".to_string(),
            rename_component: "M".to_string(),
        }
    }

    #[test]
    fn rejects_empty_duplicate_and_unsafe_slugs() {
        let path = Path::new("manifest.json");
        assert!(validate_specs(path, &[]).is_err());
        assert!(validate_specs(path, &[spec("same"), spec("same")]).is_err());
        assert!(validate_specs(path, &[spec("../outside")]).is_err());
        assert!(validate_specs(path, &[spec("row\tbreak")]).is_err());
    }
}
