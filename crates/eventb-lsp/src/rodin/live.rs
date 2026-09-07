//! Merge Rodin's *unsaved* model edits into the open buffer.
//!
//! The Rodin Editor writes each keystroke through to the Rodin database before
//! any save, so the bridge plug-in can see a rename the moment it is typed and
//! push the component's in-memory XML as a `model/dirty` notification. This
//! module turns such a push into an edit of the `.eventb` buffer, through the
//! same 3-way merge the file watcher uses.
//!
//! What it must not do is disturb the ancestor that merge runs against. The
//! provisional ancestor this module keeps for that is the whole design.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use parking_lot::Mutex;

use super::bridge::{self, Bridge};
use super::model_sync::{self, MergeOutcome};
use crate::lsp_types::Url;
use crate::server::Analyzer;

/// The ancestor a live merge runs against, per source file.
///
/// The persisted base under `<workspace>/.base` is the text the buffer and
/// Rodin's *saved* state last agreed on. Rodin's unsaved state is a third
/// replica at its own vintage, and a 3-way merge needs one ancestor per
/// replica, so the live flow keeps its own here rather than moving the
/// persisted one. Moving that one would be worse than untidy: it would leave
/// the persisted base describing text no `.bum` on disk produces, and the next
/// disk batch would rebuild "theirs" from the stale component, find `ours`
/// equal to base, and fast-forward the live edit straight back out of the
/// user's buffer.
///
/// Each entry remembers the persisted base it was seeded from, which is what
/// hands the file back when the disk flow takes it over. A save that merges,
/// or a build, moves the persisted base; the next push sees it has moved and
/// starts again from it. That is why nothing outside this module has to know
/// this state exists.
#[derive(Default)]
struct ProvisionalBases {
    bases: Mutex<HashMap<PathBuf, Ancestor>>,
}

/// One file's live ancestor, and the persisted base it was seeded from.
struct Ancestor {
    seeded_from: String,
    text: String,
}

impl ProvisionalBases {
    /// The ancestor for `source`, given the persisted base as it stands now.
    fn ancestor(&self, source: &Path, persisted: &str) -> String {
        let mut bases = self.bases.lock();
        if let Some(held) = bases.get(source)
            && held.seeded_from == persisted
        {
            return held.text.clone();
        }
        bases.insert(
            source.to_path_buf(),
            Ancestor {
                seeded_from: persisted.to_string(),
                text: persisted.to_string(),
            },
        );
        persisted.to_string()
    }

    /// Record the text a merge just applied, so the next one builds on it.
    fn advance(&self, source: &Path, text: &str) {
        if let Some(held) = self.bases.lock().get_mut(source) {
            held.text = text.to_string();
        }
    }
}

/// What one `model/dirty` push did, for the caller's logging.
enum Applied {
    /// The push carried nothing the buffer does not already say.
    Unchanged,
    /// The buffer now has Rodin's edit.
    Edited,
    /// Both sides changed the same lines. The buffer is untouched and the
    /// ancestor is not advanced, so the next push tries again.
    Deferred,
}

/// Merge one component's unsaved XML into the source file that produced it.
///
/// Errors are for the log, not the user: a push that cannot be placed (no
/// manifest, an unknown component, an unreadable source) means live sync sits
/// this one out, and the save-driven path still works.
async fn apply_dirty(
    workspace_dir: &Path,
    project: &str,
    xml_name: &str,
    xml: &str,
    bases: &ProvisionalBases,
    analyzer: &Analyzer,
) -> Result<Applied, String> {
    let manifest = model_sync::load_manifest(workspace_dir, project)
        .ok_or_else(|| format!("no build manifest for {project}"))?;
    let relative = manifest
        .source_for(xml_name)
        .ok_or_else(|| format!("{xml_name} is not in the build manifest"))?
        .to_path_buf();

    let absolute = manifest.source_root.join(&relative);
    let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    let (target, ours) = analyzer
        .source_text(&absolute)
        .ok_or_else(|| format!("cannot read {}", absolute.display()))?;

    let persisted = std::fs::read_to_string(model_sync::base_source_path(
        workspace_dir,
        &manifest.project_name,
        &relative,
    ))
    .map_err(|e| format!("no base snapshot for {}: {e}", relative.display()))?;
    let base = bases.ancestor(&absolute, &persisted);

    // Parsing, printing and merging a whole source file runs once per keystroke
    // typed in Rodin, so it goes to the blocking pool just as the disk-driven
    // merge does rather than holding a runtime worker.
    let changed: BTreeSet<String> = [xml_name.to_string()].into();
    let printer = analyzer.printer();
    let xml_name = xml_name.to_string();
    let xml = xml.to_string();
    let outcome = tokio::task::spawn_blocking(move || {
        model_sync::merge_source_file(
            &manifest,
            &relative,
            &changed,
            &base,
            &ours,
            &printer,
            |name| (name == xml_name).then(|| xml.clone()),
        )
    })
    .await
    .map_err(|e| format!("the merge task failed: {e}"))??;

    match outcome {
        MergeOutcome::Unchanged => Ok(Applied::Unchanged),
        // Markers appearing under the cursor mid-keystroke would be worse than
        // waiting: leave the buffer alone and let the next push try again.
        MergeOutcome::Conflict(_) => Ok(Applied::Deferred),
        MergeOutcome::FastForward(text) | MergeOutcome::Merged(text) => {
            let echo = target.clone();
            analyzer.apply_source_text(&absolute, target, &text).await?;
            bases.advance(&absolute, &text);
            settle(analyzer, echo).await;
            Ok(Applied::Edited)
        }
    }
}

/// How long to let the client's echo of an applied edit catch up.
const SETTLE_TIMEOUT: Duration = Duration::from_millis(500);

/// Wait until the client's echo of an applied edit has landed.
///
/// `apply_source_text` edits through the *client*, which owns the buffer, and
/// this side only learns the new text when the client sends it back as a
/// `didChange`. Until that lands, reading the buffer yields the text from
/// before the edit. The next push would then merge an ancestor that holds the
/// edit against an "ours" that does not, which reads as the user having
/// reverted it, and a clean merge would carry that revert into the buffer.
///
/// The version is the signal rather than the text, because it is a lookup
/// instead of a copy of the whole rope, and any echo at all bumps it. If none
/// comes, the merge is left no worse off than before this wait existed.
async fn settle(analyzer: &Analyzer, target: Option<(Url, i32)>) {
    let Some((uri, applied_to)) = target else {
        // Written straight to disk, so the next read already sees it.
        return;
    };
    // A document closed since the edit reads as `None`, which is not the
    // version applied to, so the wait ends there too.
    let echoed = tokio::time::timeout(SETTLE_TIMEOUT, async {
        while analyzer.document_version(&uri) == Some(applied_to) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if echoed.is_err() {
        tracing::info!("{uri} did not echo the applied edit in time");
    }
}

/// A held bridge connection feeding Rodin's unsaved edits into the buffers.
///
/// Unlike the per-operation connections the lens makes, this one stays open,
/// because `model/dirty` arrives unprompted. Dropping it stops the task and
/// closes the socket, which is also how the plug-in learns nobody is
/// listening any more.
pub struct LiveSyncManager {
    workspace_dir: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LiveSyncManager {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl LiveSyncManager {
    /// The workspace this manager follows.
    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    /// Whether the connection is still there. The loop ends when the plug-in
    /// stops pushing — a Rodin that quit — and a manager past that point
    /// carries nothing, so the caller must not treat it as live sync running.
    pub fn is_running(&self) -> bool {
        !self.task.is_finished()
    }

    /// Connect, subscribe, and apply what arrives until the connection ends.
    ///
    /// Fails when no bridge is published, when it is too old to push, or when
    /// it refuses the subscription. All three mean live sync is simply off for
    /// this session; the save-driven path is untouched either way.
    pub(crate) async fn start(workspace_dir: PathBuf, analyzer: Analyzer) -> Result<Self, String> {
        let (bridge, mut pushes) = Bridge::connect_listening(&workspace_dir).await?;
        // `model/dirty` ships with `model/subscribe`, so one check covers the
        // pair; a plug-in that cannot be subscribed to cannot push either.
        if !bridge.supports(bridge::SUBSCRIBE) {
            return Err(format!(
                "the Rodin {} bridge does not offer {}",
                bridge.rodin_version(),
                bridge::SUBSCRIBE
            ));
        }
        bridge.subscribe().await?;

        // A fresh connection is a fresh Rodin, so it starts with no ancestors:
        // nothing a previous session held describes anything now.
        let bases = ProvisionalBases::default();
        let task = {
            let workspace_dir = workspace_dir.clone();
            tokio::spawn(async move {
                // Held so the connection outlives the loop that feeds it.
                let _bridge = bridge;
                while let Some(push) = pushes.recv().await {
                    // Every push carries the component's whole XML, so within
                    // one burst only the newest per component is worth
                    // merging. Typing outruns a merge easily; without this the
                    // queue grows and the buffer lags a keystroke behind for
                    // every superseded push it works through.
                    let latest: BTreeMap<(String, String), String> = std::iter::once(push)
                        .chain(std::iter::from_fn(|| pushes.try_recv().ok()))
                        .filter(|push| push.method == bridge::DIRTY)
                        .filter_map(|push| {
                            let (Some(project), Some(file), Some(xml)) = (
                                push.params["project"].as_str(),
                                push.params["file"].as_str(),
                                push.params["xml"].as_str(),
                            ) else {
                                tracing::info!("ignoring a malformed {}", bridge::DIRTY);
                                return None;
                            };
                            Some(((project.to_string(), file.to_string()), xml.to_string()))
                        })
                        .collect();
                    for ((project, file), xml) in latest {
                        match apply_dirty(&workspace_dir, &project, &file, &xml, &bases, &analyzer)
                            .await
                        {
                            Ok(Applied::Edited) => {
                                tracing::debug!("merged Rodin's unsaved edit to {file}");
                            }
                            Ok(Applied::Deferred) => {
                                tracing::info!(
                                    "{file} conflicts with the buffer; left alone until it settles"
                                );
                            }
                            Ok(Applied::Unchanged) => {}
                            Err(message) => {
                                tracing::info!("unsaved edit to {file} not synced: {message}")
                            }
                        }
                    }
                }
                tracing::info!("the Rodin bridge stopped pushing model changes");
            })
        };

        Ok(Self {
            workspace_dir,
            task,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The persisted base is the handover signal: while it stands still the
    /// live ancestor accumulates, and the moment it moves (a save merged, or a
    /// build rewrote it) the file goes back to the disk flow. Nothing outside
    /// this module has to tell it so.
    #[test]
    fn an_ancestor_accumulates_until_the_persisted_base_moves() {
        let bases = ProvisionalBases::default();
        let file = Path::new("/src/m.eventb");

        assert_eq!(bases.ancestor(file, "as built"), "as built");
        bases.advance(file, "after a live edit");
        assert_eq!(
            bases.ancestor(file, "as built"),
            "after a live edit",
            "the next push builds on what the buffer already has"
        );

        // The disk flow advanced the persisted base, so the held ancestor
        // describes a vintage nobody is on any more.
        assert_eq!(
            bases.ancestor(file, "as merged on save"),
            "as merged on save",
            "a moved persisted base reseeds"
        );
        assert_eq!(
            bases.ancestor(file, "as merged on save"),
            "as merged on save",
            "and the reseeded ancestor is the one now held"
        );
    }

    #[test]
    fn advancing_a_file_with_no_ancestor_does_nothing() {
        // `advance` only ever follows `ancestor`, but it must not invent an
        // entry whose seed nothing established.
        let bases = ProvisionalBases::default();
        let file = Path::new("/src/m.eventb");
        bases.advance(file, "text");
        assert_eq!(bases.ancestor(file, "as built"), "as built");
    }

    #[test]
    fn ancestors_are_kept_per_file() {
        let bases = ProvisionalBases::default();
        let (a, b) = (Path::new("/src/a.eventb"), Path::new("/src/b.eventb"));
        bases.ancestor(a, "a built");
        bases.ancestor(b, "b built");
        bases.advance(a, "a edited");

        assert_eq!(bases.ancestor(a, "a built"), "a edited");
        assert_eq!(bases.ancestor(b, "b built"), "b built");
    }
}
