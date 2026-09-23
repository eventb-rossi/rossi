//! Wire-level regression tests that drive the real `LspService` end to end,
//! gathered into one binary (each `tests/*.rs` file links its own executable):
//! diagnostics debouncing, the disk-backed workspace symbol index, and the
//! `rossi/operatorTable` custom request.

use serde_json::Value;
use std::path::{Path, PathBuf};
use tower_lsp_server::jsonrpc::Request;

fn notification(method: &'static str, params: Value) -> Request {
    Request::build(method).params(params).finish()
}

/// Read server-to-client messages until the next one of `method`.
async fn next_message(
    messages: &mut (impl futures::StreamExt<Item = Request> + Unpin),
    method: &str,
    timeout: std::time::Duration,
) -> Option<Value> {
    while let Ok(Some(req)) = tokio::time::timeout(timeout, messages.next()).await {
        if req.method() == method {
            return req.params().cloned();
        }
    }
    None
}

/// The next published diagnostics batch, as the raw `diagnostics` array.
async fn next_published_diagnostics(
    messages: &mut (impl futures::StreamExt<Item = Request> + Unpin),
) -> Vec<Value> {
    let params = next_message(
        messages,
        "textDocument/publishDiagnostics",
        std::time::Duration::from_secs(5),
    )
    .await
    .expect("the server must publish diagnostics");
    params["diagnostics"]
        .as_array()
        .expect("diagnostics must be an array")
        .clone()
}

/// Read server-to-client messages until the next `window/showMessage`.
async fn next_show_message(
    messages: &mut (impl futures::StreamExt<Item = Request> + Unpin),
    timeout: std::time::Duration,
) -> Option<Value> {
    next_message(messages, "window/showMessage", timeout).await
}

/// Read server-to-client messages until the next `window/logMessage`.
async fn next_log_message(
    messages: &mut (impl futures::StreamExt<Item = Request> + Unpin),
    timeout: std::time::Duration,
) -> Option<Value> {
    next_message(messages, "window/logMessage", timeout).await
}

/// A uniquely-named workspace directory under the test target tmpdir,
/// removed again on drop.
struct TempWorkspace(PathBuf);

impl TempWorkspace {
    fn new(prefix: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl AsRef<Path> for TempWorkspace {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

mod debounce {
    //! Wire-level regression test for `diagnostics.debounceMs`.
    //!
    //! A burst of `textDocument/didChange` notifications must coalesce into a single
    //! `textDocument/publishDiagnostics` for the final version, rather than one
    //! publish per keystroke. Driving the real `LspService` exercises the debounced
    //! `tokio::spawn` path end to end (a unit test calling the handler would bypass
    //! the runtime that runs the deferred analysis). Each edit's task self-skips at
    //! wake-up unless its version is still the document's latest, so only the final
    //! edit of a burst analyzes.

    use super::notification;
    use eventb_lsp::server::RossiLanguageServer;
    use futures::StreamExt;
    use serde_json::{Value, json};
    use std::time::Duration;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    const DEBOUNCE_MS: u64 = 120;
    const URI: &str = "file:///debounce.eventb";

    /// Read server-to-client messages until the next `publishDiagnostics`, or return
    /// `None` if none arrives within `timeout` (the channel goes quiet).
    async fn next_publish(
        messages: &mut (impl StreamExt<Item = Request> + Unpin),
        timeout: Duration,
    ) -> Option<Value> {
        while let Ok(Some(req)) = tokio::time::timeout(timeout, messages.next()).await {
            if req.method() == "textDocument/publishDiagnostics" {
                return req.params().cloned();
            }
        }
        None
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rapid_edits_publish_diagnostics_once() {
        let (mut service, mut messages) = LspService::build(RossiLanguageServer::new).finish();

        // Initialize with a short, explicit debounce window.
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "initializationOptions": { "diagnostics": { "debounceMs": DEBOUNCE_MS } }
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        // Open a document with a broken invariant. `didOpen` analyzes inline (not
        // debounced), so its diagnostics publish promptly.
        let open = notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": URI,
                    "languageId": "eventb",
                    "version": 1,
                    "text": "MACHINE m\nINVARIANTS\n@i x ∈\nEND\n"
                }
            }),
        );
        service.ready().await.unwrap().call(open).await.unwrap();

        let opened = next_publish(&mut messages, Duration::from_millis(500))
            .await
            .expect("didOpen publishes diagnostics inline");
        assert_eq!(opened["version"], json!(1), "open publishes for version 1");

        // Fire several edits back to back, faster than the debounce window. Each
        // bumps the document version, so the earlier edits' tasks will find
        // themselves superseded at wake-up.
        for version in 2..=5 {
            let change = notification(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": URI, "version": version },
                    "contentChanges": [
                        { "text": format!("MACHINE m\nINVARIANTS\n@i x ∈ {version}\nEND\n") }
                    ]
                }),
            );
            service.ready().await.unwrap().call(change).await.unwrap();
        }

        // Let the tasks fire, then drain. Exactly one publish — for the final
        // version — should have arrived; the earlier four found a newer version at
        // wake-up and bowed out.
        tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS + 150)).await;

        let mut publishes = Vec::new();
        while let Some(params) = next_publish(&mut messages, Duration::from_millis(100)).await {
            publishes.push(params);
        }

        assert_eq!(
            publishes.len(),
            1,
            "a burst of edits collapses to one diagnostics publish, got {publishes:?}"
        );
        assert_eq!(
            publishes[0]["version"],
            json!(5),
            "the surviving publish is for the latest version"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn zero_debounce_publishes_each_edit_inline() {
        let (mut service, mut messages) = LspService::build(RossiLanguageServer::new).finish();

        // A zero window opts out of debouncing: each edit analyzes inline.
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "initializationOptions": { "diagnostics": { "debounceMs": 0 } }
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        let open = notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": URI,
                    "languageId": "eventb",
                    "version": 1,
                    "text": "MACHINE m\nINVARIANTS\n@i x ∈\nEND\n"
                }
            }),
        );
        service.ready().await.unwrap().call(open).await.unwrap();
        let opened = next_publish(&mut messages, Duration::from_millis(500)).await;
        assert_eq!(opened.expect("open publishes")["version"], json!(1));

        // Each change publishes synchronously, in order — no coalescing.
        for version in 2..=3 {
            let change = notification(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": URI, "version": version },
                    "contentChanges": [
                        { "text": format!("MACHINE m\nINVARIANTS\n@i x ∈ {version}\nEND\n") }
                    ]
                }),
            );
            service.ready().await.unwrap().call(change).await.unwrap();
            let published = next_publish(&mut messages, Duration::from_millis(500))
                .await
                .expect("each inline edit publishes diagnostics");
            assert_eq!(published["version"], json!(version));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn debounce_does_not_cross_document_lifecycles() {
        const LIFECYCLE_DEBOUNCE_MS: u64 = 200;

        let (mut service, mut messages) = LspService::build(RossiLanguageServer::new).finish();
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "initializationOptions": {
                    "diagnostics": { "debounceMs": LIFECYCLE_DEBOUNCE_MS }
                }
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        let open = |version: i32, name: &str| {
            notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": URI,
                        "languageId": "eventb",
                        "version": version,
                        "text": format!("CONTEXT {name}\nEND\n")
                    }
                }),
            )
        };
        let change = |version: i32, name: &str| {
            notification(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": URI, "version": version },
                    "contentChanges": [{ "text": format!("CONTEXT {name}\nEND\n") }]
                }),
            )
        };

        service
            .ready()
            .await
            .unwrap()
            .call(open(0, "first"))
            .await
            .unwrap();
        next_publish(&mut messages, Duration::from_millis(500))
            .await
            .expect("first open publishes");
        service
            .ready()
            .await
            .unwrap()
            .call(change(1, "first_changed"))
            .await
            .unwrap();

        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didClose",
                json!({ "textDocument": { "uri": URI } }),
            ))
            .await
            .unwrap();
        next_publish(&mut messages, Duration::from_millis(500))
            .await
            .expect("close clears diagnostics");
        service
            .ready()
            .await
            .unwrap()
            .call(open(0, "second"))
            .await
            .unwrap();
        next_publish(&mut messages, Duration::from_millis(500))
            .await
            .expect("second open publishes");

        tokio::time::sleep(Duration::from_millis(100)).await;
        service
            .ready()
            .await
            .unwrap()
            .call(change(1, "second_changed"))
            .await
            .unwrap();

        // Lifecycle A's version-1 timer wakes during this interval. It must not
        // analyze lifecycle B merely because B has independently reached version 1.
        tokio::time::sleep(Duration::from_millis(130)).await;
        assert!(
            next_publish(&mut messages, Duration::from_millis(20))
                .await
                .is_none(),
            "an old lifecycle's debounce task must not publish for the new document"
        );

        let published = next_publish(&mut messages, Duration::from_millis(150))
            .await
            .expect("the current lifecycle publishes after its own debounce");
        assert_eq!(published["version"], json!(1));
    }
}

mod workspace_symbols {
    //! Wire-level regressions for the disk-backed workspace symbol index.

    use super::{TempWorkspace, notification};
    use eventb_lsp::lsp_types::Uri;
    use eventb_lsp::server::RossiLanguageServer;
    use futures::StreamExt;
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    #[tokio::test(flavor = "current_thread")]
    async fn disk_symbols_are_overlaid_while_open_and_restored_on_close() {
        let workspace = TempWorkspace::new("workspace-symbols-test");
        let path = workspace.as_ref().join("model.eventb");
        std::fs::write(
            &path,
            "CONTEXT disk_context\nCONSTANTS\n    disk_value\nEND\n",
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&path, workspace.as_ref().join("alias.eventb")).unwrap();
        let root_uri = Uri::from_file_path(workspace.as_ref()).unwrap();
        let file_uri = Uri::from_file_path(&path).unwrap();

        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        tokio::spawn(async move { while socket.next().await.is_some() {} });
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }]
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification("initialized", json!({})))
            .await
            .unwrap();

        macro_rules! symbol_names {
            ($id:expr, $query:expr) => {{
                let request = Request::build("workspace/symbol")
                    .id($id)
                    .params(json!({ "query": $query }))
                    .finish();
                let response = service
                    .ready()
                    .await
                    .unwrap()
                    .call(request)
                    .await
                    .unwrap()
                    .expect("workspace/symbol must produce a response");
                let (_id, result) = response.into_parts();
                result
                    .expect("workspace/symbol request must succeed")
                    .as_array()
                    .expect("workspace/symbol result must be an array")
                    .iter()
                    .map(|symbol| symbol["name"].as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            }};
        }

        assert_eq!(symbol_names!(2, "disk_context"), ["disk_context"]);
        assert_eq!(symbol_names!(3, "disk_value"), ["disk_value"]);

        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": file_uri,
                        "languageId": "eventb",
                        "version": 1,
                        "text": "CONTEXT open_context\nCONSTANTS\n    open_value\nEND\n"
                    }
                }),
            ))
            .await
            .unwrap();

        assert!(symbol_names!(4, "disk_value").is_empty());
        assert_eq!(symbol_names!(5, "open_value"), ["open_value"]);

        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didClose",
                json!({ "textDocument": { "uri": file_uri } }),
            ))
            .await
            .unwrap();

        assert_eq!(symbol_names!(6, "disk_value"), ["disk_value"]);
        assert!(symbol_names!(7, "open_value").is_empty());

        let saved_source = "CONTEXT saved_context\nCONSTANTS\n    saved_value\nEND\n";
        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": file_uri,
                        "languageId": "eventb",
                        "version": 2,
                        "text": saved_source
                    }
                }),
            ))
            .await
            .unwrap();
        std::fs::write(&path, saved_source).unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didSave",
                json!({ "textDocument": { "uri": file_uri } }),
            ))
            .await
            .unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didClose",
                json!({ "textDocument": { "uri": file_uri } }),
            ))
            .await
            .unwrap();

        assert_eq!(symbol_names!(8, "saved_value"), ["saved_value"]);
        assert!(symbol_names!(9, "disk_value").is_empty());
    }
}

mod rodin_lens {
    //! Wire-level tests for the "Open in Rodin" CodeLens + executeCommand
    //! surface: capability advertisement, lens shape, and the executeCommand
    //! path building the project on disk even when no Rodin install exists
    //! (the error must point at the `rossi.rodin.path` setting).

    use super::{TempWorkspace, next_show_message, notification};
    use eventb_lsp::lsp_types::Uri;
    use eventb_lsp::server::RossiLanguageServer;
    use futures::StreamExt;
    use serde_json::{Value, json};
    use std::time::Duration;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    const SOURCE: &str = "CONTEXT wire_ctx\nCONSTANTS\n    lo\nAXIOMS\n    @axm1 lo ∈ ℤ\nEND\n\nMACHINE wire_m\nSEES wire_ctx\nEND\n";

    #[tokio::test(flavor = "current_thread")]
    async fn advertises_capabilities_and_serves_lenses() {
        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        tokio::spawn(async move { while socket.next().await.is_some() {} });

        let init = Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {} }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(init)
            .await
            .unwrap()
            .expect("initialize responds");
        let (_id, result) = response.into_parts();
        let capabilities = &result.expect("initialize succeeds")["capabilities"];
        assert_eq!(capabilities["codeLensProvider"]["resolveProvider"], false);
        assert_eq!(capabilities["inlayHintProvider"], true);
        assert_eq!(
            capabilities["executeCommandProvider"]["commands"],
            json!([
                "rossi.rodin.open",
                "rossi.animate.check",
                "rossi.animate.po"
            ])
        );

        let uri = "file:///wire.eventb";
        let open = notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "eventb",
                    "version": 1,
                    "text": SOURCE
                }
            }),
        );
        service.ready().await.unwrap().call(open).await.unwrap();

        let lens_request = Request::build("textDocument/codeLens")
            .id(2)
            .params(json!({ "textDocument": { "uri": uri } }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(lens_request)
            .await
            .unwrap()
            .expect("codeLens responds");
        let (_id, result) = response.into_parts();
        let lenses = result.expect("codeLens succeeds");
        let lenses = lenses.as_array().expect("codeLens result is an array");
        // One rodin lens per component, plus the two animate lenses on the
        // machine (contexts cannot be animated).
        assert_eq!(lenses.len(), 4, "unexpected lens set: {lenses:?}");
        for lens in &lenses[..2] {
            assert_eq!(lens["command"]["title"], "Open in Rodin");
            assert_eq!(lens["command"]["command"], "rossi.rodin.open");
            assert_eq!(lens["command"]["arguments"], json!([uri]));
        }
        // The context header is on line 0, the machine header on line 7.
        assert_eq!(lenses[0]["range"]["start"]["line"], 0);
        assert_eq!(lenses[1]["range"]["start"]["line"], 7);
        for (lens, (title, command)) in lenses[2..].iter().zip([
            ("Model-check", "rossi.animate.check"),
            ("Disprove POs", "rossi.animate.po"),
        ]) {
            assert_eq!(lens["command"]["title"], title);
            assert_eq!(lens["command"]["command"], command);
            assert_eq!(lens["command"]["arguments"], json!([uri, "wire_m"]));
            assert_eq!(lens["range"]["start"]["line"], 7);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_command_builds_project_and_reports_missing_rodin() {
        let workspace = TempWorkspace::new("rodin-lens-test");
        let source_path = workspace.as_ref().join("model.eventb");
        std::fs::write(&source_path, SOURCE).unwrap();
        let rodin_workspace = workspace.as_ref().join("rodin-ws");
        let root_uri = Uri::from_file_path(workspace.as_ref()).unwrap();
        let file_uri = Uri::from_file_path(&source_path).unwrap();

        let (mut service, mut messages) = LspService::build(RossiLanguageServer::new).finish();
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "initializationOptions": {
                    "rodin": {
                        "path": "/nonexistent/rodin-install",
                        "workspace": rodin_workspace.to_str().unwrap()
                    }
                }
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        // Open the file with an *edited* buffer: the overlay (not the disk
        // file) must be what the build reads.
        let open = notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": file_uri,
                    "languageId": "eventb",
                    "version": 1,
                    "text": SOURCE.replace("wire_ctx", "buffer_ctx")
                }
            }),
        );
        service.ready().await.unwrap().call(open).await.unwrap();

        let execute = Request::build("workspace/executeCommand")
            .id(2)
            .params(json!({
                "command": "rossi.rodin.open",
                "arguments": [file_uri.as_str().to_owned()]
            }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(execute)
            .await
            .unwrap()
            .expect("executeCommand responds");
        let (_id, result) = response.into_parts();
        assert_eq!(result.expect("executeCommand succeeds"), Value::Null);

        // The spawned flow builds the project, then fails on the bogus Rodin
        // path with a message pointing at the setting.
        let message = next_show_message(&mut messages, Duration::from_secs(10))
            .await
            .expect("the flow reports through window/showMessage");
        let text = message["message"].as_str().unwrap();
        assert!(
            text.contains("was not found") && text.contains("rossi.rodin.path"),
            "unexpected message: {text}"
        );

        let project_dir = rodin_workspace.join(
            workspace
                .as_ref()
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap(),
        );
        assert!(project_dir.join(".project").is_file());
        assert!(
            project_dir.join("buffer_ctx.buc").is_file(),
            "the open buffer's text must win over the disk file"
        );
        assert!(project_dir.join("wire_m.bum").is_file());
        assert!(project_dir.join("wire_m.bpo").is_file());
        assert!(project_dir.join("wire_m.bps").is_file());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_command_rejects_unknown_commands() {
        let (mut service, _socket) = LspService::build(RossiLanguageServer::new).finish();
        let init = Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {} }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        let execute = Request::build("workspace/executeCommand")
            .id(2)
            .params(json!({ "command": "rossi.rodin.unknown", "arguments": [] }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(execute)
            .await
            .unwrap()
            .expect("executeCommand responds");
        let (_id, result) = response.into_parts();
        assert!(result.is_err(), "unknown commands must be rejected");
    }
}

mod inlay_hints {
    //! Wire-level tests for `textDocument/inlayHint`: the declaration type
    //! hint round-trip, and `rossi.inlayHints.enabled=false` arriving over
    //! `workspace/didChangeConfiguration` turning the response into null.

    use super::notification;
    use eventb_lsp::server::RossiLanguageServer;
    use futures::StreamExt;
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    const SOURCE: &str = "CONTEXT wire_ctx\nCONSTANTS\n    lo\nAXIOMS\n    @axm1 lo ∈ ℤ\nEND\n";

    #[tokio::test(flavor = "current_thread")]
    async fn serves_declaration_type_hints_until_disabled() {
        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        tokio::spawn(async move { while socket.next().await.is_some() {} });

        let init = Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {} }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        let uri = "file:///wire-hints.eventb";
        let open = notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "eventb",
                    "version": 1,
                    "text": SOURCE
                }
            }),
        );
        service.ready().await.unwrap().call(open).await.unwrap();

        let hint_request = |id: i64| {
            Request::build("textDocument/inlayHint")
                .id(id)
                .params(json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 99, "character": 0 }
                    }
                }))
                .finish()
        };

        let response = service
            .ready()
            .await
            .unwrap()
            .call(hint_request(2))
            .await
            .unwrap()
            .expect("inlayHint responds");
        let (_id, result) = response.into_parts();
        let hints = result.expect("inlayHint succeeds");
        // The constant `lo` is declared on line 2, columns 4-6.
        assert_eq!(
            hints,
            json!([{
                "position": { "line": 2, "character": 6 },
                "label": ": ℤ",
                "kind": 1
            }]),
        );

        let disable = notification(
            "workspace/didChangeConfiguration",
            json!({
                "settings": { "rossi": { "inlayHints": { "enabled": false } } }
            }),
        );
        service.ready().await.unwrap().call(disable).await.unwrap();

        let response = service
            .ready()
            .await
            .unwrap()
            .call(hint_request(3))
            .await
            .unwrap()
            .expect("inlayHint responds");
        let (_id, result) = response.into_parts();
        assert_eq!(result.expect("inlayHint succeeds"), json!(null));
    }
}

mod operator_convention {
    //! Wire-level test for the `rossi.format.enforceUnicode` advisory: on
    //! under the initialization options, its diagnostics are published when
    //! the document opens; switching to the ASCII convention or turning it
    //! off through `workspace/didChangeConfiguration` republishes the
    //! document without them, with no edit in between.

    use super::{next_published_diagnostics, notification};
    use eventb_lsp::server::RossiLanguageServer;
    use futures::StreamExt;
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    const SOURCE: &str = "MACHINE m\nVARIABLES x\nINVARIANTS\n    @inv1 x : NAT\nEND\n";

    /// The `(code, message)` pairs of the next published diagnostics batch.
    async fn next_diagnostics(
        messages: &mut (impl StreamExt<Item = Request> + Unpin),
    ) -> Vec<(String, String)> {
        next_published_diagnostics(messages)
            .await
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic["code"].as_str().unwrap_or_default().to_string(),
                    diagnostic["message"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ascii_operators_are_flagged_until_the_setting_is_turned_off() {
        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        let (sender, mut messages) = futures::channel::mpsc::unbounded();
        tokio::spawn(async move {
            while let Some(request) = socket.next().await {
                if sender.unbounded_send(request).is_err() {
                    break;
                }
            }
        });

        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "initializationOptions": { "format": { "enforceUnicode": true } }
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        let open = notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": "file:///convention.eventb",
                    "languageId": "eventb",
                    "version": 1,
                    "text": SOURCE
                }
            }),
        );
        service.ready().await.unwrap().call(open).await.unwrap();

        assert_eq!(
            next_diagnostics(&mut messages).await,
            [
                (
                    "ascii-operator".to_string(),
                    "use `∈` instead of ASCII `:`".to_string()
                ),
                (
                    "ascii-operator".to_string(),
                    "use `ℕ` instead of ASCII `NAT`".to_string()
                ),
            ]
        );

        let ascii_convention = notification(
            "workspace/didChangeConfiguration",
            json!({
                "settings": { "rossi": { "format": { "useUnicode": false, "enforceUnicode": true } } }
            }),
        );
        service
            .ready()
            .await
            .unwrap()
            .call(ascii_convention)
            .await
            .unwrap();

        assert!(
            next_diagnostics(&mut messages).await.is_empty(),
            "the advisory must not fire under the ASCII convention, whose fix-all would revert its fixes"
        );

        let disable = notification(
            "workspace/didChangeConfiguration",
            json!({
                "settings": { "rossi": { "format": { "enforceUnicode": false } } }
            }),
        );
        service.ready().await.unwrap().call(disable).await.unwrap();

        assert!(
            next_diagnostics(&mut messages).await.is_empty(),
            "turning the advisory off must republish the document without it"
        );
    }
}

mod animate_lens {
    //! Wire-level tests for the eventb-animate executeCommand surface: the
    //! spawned flow must fail fast with a message naming the
    //! `rossi.animate.path` setting when the configured tool is missing, and
    //! malformed arguments must be rejected at the JSON-RPC layer.

    use super::{next_log_message, next_show_message, notification};
    use eventb_lsp::server::RossiLanguageServer;
    use futures::StreamExt;
    use serde_json::{Value, json};
    use std::time::Duration;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    const SOURCE: &str = "MACHINE animate_m\nEND\n";
    const URI: &str = "file:///animate.eventb";

    async fn initialized_service(
        path: &str,
    ) -> (
        LspService<RossiLanguageServer>,
        impl StreamExt<Item = Request> + Unpin,
    ) {
        let (mut service, messages) = LspService::build(RossiLanguageServer::new).finish();
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "initializationOptions": { "animate": { "path": path } }
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        let open = notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": URI,
                    "languageId": "eventb",
                    "version": 1,
                    "text": SOURCE
                }
            }),
        );
        service.ready().await.unwrap().call(open).await.unwrap();
        (service, messages)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_command_reports_missing_tool_naming_the_setting() {
        let (mut service, mut messages) = initialized_service("/nonexistent/eventb-animate").await;

        let execute = Request::build("workspace/executeCommand")
            .id(2)
            .params(json!({
                "command": "rossi.animate.check",
                "arguments": [URI, "animate_m"]
            }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(execute)
            .await
            .unwrap()
            .expect("executeCommand responds");
        let (_id, result) = response.into_parts();
        assert_eq!(result.expect("executeCommand succeeds"), Value::Null);

        // The failure is logged before it is toasted, so the log line comes
        // first on the wire.
        let log = next_log_message(&mut messages, Duration::from_secs(10))
            .await
            .expect("the failure is logged through window/logMessage");
        assert_eq!(log["type"], 1, "ERROR level: {log}");
        let log_text = log["message"].as_str().unwrap();
        assert!(
            log_text.contains("was not found"),
            "unexpected log line: {log_text}"
        );

        let message = next_show_message(&mut messages, Duration::from_secs(10))
            .await
            .expect("the flow reports through window/showMessage");
        let text = message["message"].as_str().unwrap();
        assert!(
            text.contains("was not found") && text.contains("rossi.animate.path"),
            "unexpected message: {text}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_command_rejects_missing_machine_argument() {
        let (mut service, _messages) = initialized_service("/nonexistent/eventb-animate").await;

        let execute = Request::build("workspace/executeCommand")
            .id(2)
            .params(json!({
                "command": "rossi.animate.po",
                "arguments": [URI]
            }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(execute)
            .await
            .unwrap()
            .expect("executeCommand responds");
        let (_id, result) = response.into_parts();
        let error = result.expect_err("a missing machine argument is invalid");
        assert!(
            error.message.contains("machine name"),
            "unexpected error: {}",
            error.message
        );
    }

    /// The next diagnostics published for `uri`.
    async fn next_diagnostics_for(
        messages: &mut (impl StreamExt<Item = Request> + Unpin),
        uri: &Value,
    ) -> Vec<Value> {
        loop {
            let params = super::next_message(
                messages,
                "textDocument/publishDiagnostics",
                Duration::from_secs(20),
            )
            .await
            .expect("diagnostics must be published");
            if params["uri"] == *uri {
                return params["diagnostics"].as_array().unwrap().clone();
            }
        }
    }

    fn from_animate(diagnostics: &[Value]) -> bool {
        diagnostics
            .iter()
            .any(|d| d["source"] == json!("eventb-animate"))
    }

    async fn send(
        service: &mut LspService<RossiLanguageServer>,
        method: &'static str,
        params: Value,
    ) {
        service
            .ready()
            .await
            .unwrap()
            .call(notification(method, params))
            .await
            .unwrap();
    }

    /// A model-check error describes the model it ran on: saving a file of
    /// that model drops it, saving any other file keeps it.
    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn saving_a_file_of_the_model_drops_its_model_check_error() {
        use super::TempWorkspace;
        use eventb_lsp::lsp_types::Uri;
        use std::os::unix::fs::PermissionsExt;

        let workspace = TempWorkspace::new("animate-save");
        let root = workspace.as_ref();
        // What eventb-animate reports when ProB finds no values for the
        // constants.
        let tool = root.join("eventb-animate");
        std::fs::write(
            &tool,
            concat!(
                "#!/bin/sh\n",
                "echo '{\"formatVersion\": 4, \"tool\": \"eventb-animate\", ",
                "\"command\": \"check\", \"status\": \"error\", ",
                "\"message\": \"No feasible constant setup exists.\"}'\n",
                "exit 70\n",
            ),
        )
        .unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let files = [
            (
                "C.eventb",
                "CONTEXT C\nCONSTANTS\n    c\nAXIOMS\n    @axm1 c \u{2208} \u{2115}\nEND\n",
            ),
            ("M.eventb", "MACHINE M\nSEES C\nEND\n"),
            ("U.eventb", "CONTEXT U\nEND\n"),
        ];
        for (name, text) in files {
            std::fs::write(root.join(name), text).unwrap();
        }
        let uri = |name: &str| json!(Uri::from_file_path(root.join(name)).unwrap());

        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        let (sender, mut messages) = futures::channel::mpsc::unbounded();
        tokio::spawn(async move {
            while let Some(request) = socket.next().await {
                let _ = sender.unbounded_send(request);
            }
        });
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "workspaceFolders": [{ "uri": Uri::from_file_path(root).unwrap(), "name": "test" }],
                "initializationOptions": { "animate": { "path": tool } }
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        send(&mut service, "initialized", json!({})).await;
        for (name, text) in files {
            send(
                &mut service,
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri(name),
                        "languageId": "eventb",
                        "version": 1,
                        "text": text,
                    }
                }),
            )
            .await;
        }

        let execute = Request::build("workspace/executeCommand")
            .id(2)
            .params(json!({
                "command": "rossi.animate.check",
                "arguments": [uri("M.eventb"), "M"]
            }))
            .finish();
        service.ready().await.unwrap().call(execute).await.unwrap();
        while !from_animate(&next_diagnostics_for(&mut messages, &uri("M.eventb")).await) {}

        // Saving a file the machine does not depend on keeps the error.
        send(
            &mut service,
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri("U.eventb") } }),
        )
        .await;
        send(
            &mut service,
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri("M.eventb"), "version": 2 },
                "contentChanges": [{ "text": files[1].1 }]
            }),
        )
        .await;
        assert!(
            from_animate(&next_diagnostics_for(&mut messages, &uri("M.eventb")).await),
            "saving an unrelated file must keep the model-check error"
        );

        // Saving the seen context, where the constants are fixed, drops it.
        send(
            &mut service,
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri("C.eventb") } }),
        )
        .await;
        assert!(
            !from_animate(&next_diagnostics_for(&mut messages, &uri("M.eventb")).await),
            "saving a file of the model must drop the model-check error"
        );
    }
}

mod progress_cancel {
    //! `window/workDoneProgress/cancel` reaches the server as a custom
    //! notification (the framework has no trait hook for it). Pins that the
    //! registration accepts the notification's real parameter shape, and
    //! that a token nothing tracks is ignored rather than answered with an
    //! error the client would log.

    use eventb_lsp::server::RossiLanguageServer;
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    #[tokio::test(flavor = "current_thread")]
    async fn a_cancel_for_an_untracked_token_is_accepted_silently() {
        let (mut service, _socket) = LspService::build(RossiLanguageServer::new)
            .custom_method(
                "window/workDoneProgress/cancel",
                RossiLanguageServer::work_done_progress_cancel,
            )
            .finish();
        let init = Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {} }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        let cancel = Request::build("window/workDoneProgress/cancel")
            .params(json!({ "token": "rossi-progress-0" }))
            .finish();
        let response = service.ready().await.unwrap().call(cancel).await.unwrap();
        assert!(
            response.is_none(),
            "a notification produces no response; got {response:?}"
        );
    }
}

mod operator_table {
    //! Wire-level regression test for the `rossi/operatorTable` custom request.
    //!
    //! Pins `operator_table` to a parameter-less signature: the VS Code client sends
    //! this request with no `params`, which a params-taking handler rejects (see the
    //! handler doc in `server.rs` for the tower-lsp routing detail). The test drives
    //! the real `LspService` with a params-less request so that failure is exercised
    //! end to end — a unit test calling `operator_table()` directly would bypass
    //! tower-lsp's param extraction, which is exactly where the bug lived.

    use eventb_lsp::server::RossiLanguageServer;
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    #[tokio::test(flavor = "current_thread")]
    async fn operator_table_succeeds_without_params_field() {
        let (mut service, _socket) = LspService::build(RossiLanguageServer::new)
            .custom_method("rossi/operatorTable", RossiLanguageServer::operator_table)
            .finish();

        // A real client session initializes before issuing requests.
        let init = Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {} }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        // Exactly what vscode-languageclient emits for a paramless sendRequest:
        // a request with NO `params` field (the builder omits it by default).
        let request = Request::build("rossi/operatorTable").id(2).finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap()
            .expect("custom request must produce a response");

        let (_id, result) = response.into_parts();
        let value = result.expect("rossi/operatorTable must succeed when params is absent");
        let rows = value.as_array().expect("operator table is a JSON array");
        assert!(
            rows.iter()
                .any(|row| row["ascii"] == "/=" && row["unicode"] == "≠" && row["eager"] == true),
            "operator table must carry the /= -> ≠ eager mapping; got {value}"
        );
        // `,,` is an ASCII input alias for the maplet ↦ (Rodin's keyboard); it must
        // ride along as its own eager row so the editor converts it as you type.
        assert!(
            rows.iter()
                .any(|row| row["ascii"] == ",," && row["unicode"] == "↦" && row["eager"] == true),
            "operator table must carry the ,, -> ↦ eager mapping; got {value}"
        );
    }
}

mod project_diagnostics {
    //! The project-level static check reaches the editor: a type error that
    //! no single-component pass can see is published, anchored on the
    //! offending element, and a clean model stays clean.

    use super::{TempWorkspace, next_published_diagnostics, notification};
    use eventb_lsp::lsp_types::Uri;
    use eventb_lsp::server::RossiLanguageServer;
    use serde_json::{Value, json};
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    /// `c` is a carrier-set element, so comparing it to a number cannot type.
    /// Nothing in the file is locally malformed: only the project check,
    /// which infers types across the SEES edge, can see this.
    const BAD_TYPE: &str = concat!(
        "CONTEXT typing\n",
        "SETS\n",
        "    S\n",
        "CONSTANTS\n",
        "    c\n",
        "AXIOMS\n",
        "    @axm1 c \u{2208} S\n",
        "    @axm2 c = 1\n",
        "END\n",
    );
    const GOOD_TYPE: &str = concat!(
        "CONTEXT typing\n",
        "SETS\n",
        "    S\n",
        "CONSTANTS\n",
        "    c\n",
        "AXIOMS\n",
        "    @axm1 c \u{2208} S\n",
        "END\n",
    );

    async fn diagnostics_for(text: &str) -> Vec<Value> {
        let workspace = TempWorkspace::new("project-diagnostics");
        let uri = Uri::from_file_path(workspace.as_ref().join("typing.eventb")).unwrap();

        let (mut service, mut messages) = LspService::build(RossiLanguageServer::new).finish();
        let init = Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {} }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "eventb",
                        "version": 1,
                        "text": text,
                    }
                }),
            ))
            .await
            .unwrap();

        next_published_diagnostics(&mut messages).await
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_type_error_is_published_and_anchored() {
        let diagnostics = diagnostics_for(BAD_TYPE).await;
        assert!(
            !diagnostics.is_empty(),
            "the ill-typed axiom must be reported; got {diagnostics:?}"
        );

        // Every finding must land on the offending line rather than on the
        // whole-file default range, which is what an unmapped span produces.
        assert!(
            diagnostics
                .iter()
                .all(|d| d["range"]["start"]["line"] == json!(7)),
            "findings must anchor on @axm2 (line 7); got {diagnostics:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_duplicate_or_primed_name_is_reported_once() {
        // The per-component lint and the project check both run the
        // duplicate and primed-name passes; the editor shows each finding
        // once.
        for (code, text) in [
            ("EB021", "CONTEXT dup\nSETS\n    S\nCONSTANTS\n    S\nEND\n"),
            (
                "EB022",
                "CONTEXT dup\nCONSTANTS\n    c\nAXIOMS\n    @axm1 c \u{2208} \u{2115}\n    @axm1 c \u{2265} 0\nEND\n",
            ),
            (
                "EB033",
                "CONTEXT dup\nCONSTANTS\n    c'\nAXIOMS\n    @axm1 c' \u{2208} \u{2115}\nEND\n",
            ),
        ] {
            let diagnostics = diagnostics_for(text).await;
            let reported = diagnostics
                .iter()
                .filter(|d| d["code"] == json!(code))
                .count();
            assert_eq!(reported, 1, "{code} once; got {diagnostics:?}");
        }
    }

    /// `evt` in `M1` extends `evt` in `M0` and reuses its guard label.
    const INHERITED_LABEL: &str = concat!(
        "MACHINE M0\n",
        "VARIABLES\n",
        "    x\n",
        "INVARIANTS\n",
        "    @inv1 x \u{2208} \u{2124}\n",
        "EVENTS\n",
        "    EVENT INITIALISATION\n",
        "    THEN\n",
        "        @init1 x \u{2254} 0\n",
        "    END\n",
        "    EVENT evt\n",
        "    WHERE\n",
        "        @grd1 x \u{2265} 0\n",
        "    END\n",
        "END\n",
        "MACHINE M1\n",
        "REFINES M0\n",
        "VARIABLES\n",
        "    x\n",
        "EVENTS\n",
        "    EVENT INITIALISATION EXTENDS INITIALISATION\n",
        "    END\n",
        "    EVENT evt EXTENDS evt\n",
        "    WHERE\n",
        "        @grd1 x \u{2265} 1\n",
        "    END\n",
        "END\n",
    );

    #[tokio::test(flavor = "current_thread")]
    async fn a_label_clashing_with_an_inherited_one_is_reported() {
        // Only the project check sees the abstract event, so its EB022 is
        // the one copy the editor gets.
        let diagnostics = diagnostics_for(INHERITED_LABEL).await;
        let reported = diagnostics
            .iter()
            .filter(|d| d["code"] == json!("EB022"))
            .count();
        assert_eq!(reported, 1, "EB022 once; got {diagnostics:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_well_typed_model_reports_nothing() {
        let diagnostics = diagnostics_for(GOOD_TYPE).await;
        assert!(
            diagnostics.is_empty(),
            "a well-typed context is clean; got {diagnostics:?}"
        );
    }

    /// A refinement whose abstract machine is nowhere to be found: no
    /// workspace was scanned, so the closure cannot load `base`, and `x`
    /// comes from it.
    const UNRESOLVED_PARENT: &str = concat!(
        "MACHINE concrete\n",
        "REFINES base\n",
        "VARIABLES\n",
        "    y\n",
        "INVARIANTS\n",
        "    @inv1 y = x\n",
        "END\n",
    );

    #[tokio::test(flavor = "current_thread")]
    async fn an_unloadable_dependency_reports_nothing_about_the_names_it_declares() {
        // Opening a refinement on its own must not claim its inherited
        // variables do not exist: the environment is incomplete, not wrong.
        // The missing REFINES target is the dependency graph's finding, and
        // the graph is gated on a scanned workspace.
        let diagnostics = diagnostics_for(UNRESOLVED_PARENT).await;
        assert!(
            diagnostics.is_empty(),
            "an unresolvable closure must report nothing; got {diagnostics:?}"
        );
    }
}

mod type_hierarchy {
    //! Wire-level tests for the refinement/extension type hierarchy: the
    //! capability is registered dynamically (the pinned lsp-types has no
    //! server-capability field for it), a machine's supertype is what it
    //! REFINES, and its subtypes are the machines that refine it.

    use super::{TempWorkspace, notification};
    use eventb_lsp::lsp_types::Uri;
    use eventb_lsp::server::RossiLanguageServer;
    use futures::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::{Request, Response};

    const ABSTRACT: &str = concat!(
        "MACHINE base\n",
        "VARIABLES\n",
        "    x\n",
        "INVARIANTS\n",
        "    @inv1 x \u{2208} \u{2115}\n",
        "EVENTS\n",
        "    EVENT INITIALISATION\n",
        "    THEN\n",
        "        @act1 x \u{2254} 0\n",
        "    END\n",
        "END\n",
    );
    const CONCRETE: &str = concat!(
        "MACHINE refined\n",
        "REFINES\n",
        "    base\n",
        "VARIABLES\n",
        "    x\n",
        "INVARIANTS\n",
        "    @inv1 x \u{2208} \u{2115}\n",
        "EVENTS\n",
        "    EVENT INITIALISATION\n",
        "    THEN\n",
        "        @act1 x \u{2254} 0\n",
        "    END\n",
        "END\n",
    );

    #[tokio::test(flavor = "current_thread")]
    async fn the_capability_is_registered_dynamically() {
        let workspace = TempWorkspace::new("type-hierarchy-register");
        let root_uri = Uri::from_file_path(workspace.as_ref()).unwrap();

        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();

        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                // Only the type hierarchy opts into dynamic registration, so
                // this is the sole registration the server will send.
                "capabilities": {
                    "textDocument": { "typeHierarchy": { "dynamicRegistration": true } }
                },
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }]
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        // `initialized` waits for the client's answer to
        // `client/registerCapability`, so the socket has to be served while the
        // notification is still in flight, exactly as a real client does.
        let drive_initialized = async {
            service
                .ready()
                .await
                .unwrap()
                .call(notification("initialized", json!({})))
                .await
                .unwrap();
        };
        let answer = async {
            loop {
                let request = socket
                    .next()
                    .await
                    .expect("the server must ask to register the type hierarchy");
                if request.method() != "client/registerCapability" {
                    continue;
                }
                let (_method, id, params) = request.into_parts();
                let registration =
                    &params.expect("a registration must be sent")["registrations"][0];
                assert_eq!(registration["method"], "textDocument/prepareTypeHierarchy");
                assert_eq!(
                    registration["registerOptions"]["documentSelector"][0]["pattern"],
                    "**/*.eventb",
                    "the selector must be scoped to Event-B sources"
                );
                socket
                    .send(Response::from_ok(
                        id.expect("a request carries an id"),
                        json!(null),
                    ))
                    .await
                    .unwrap();
                break;
            }
        };
        tokio::join!(drive_initialized, answer);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_machine_resolves_to_what_it_refines_and_what_refines_it() {
        let workspace = TempWorkspace::new("type-hierarchy-edges");
        std::fs::write(workspace.as_ref().join("base.eventb"), ABSTRACT).unwrap();
        let concrete_path = workspace.as_ref().join("refined.eventb");
        std::fs::write(&concrete_path, CONCRETE).unwrap();
        let root_uri = Uri::from_file_path(workspace.as_ref()).unwrap();
        let concrete_uri = Uri::from_file_path(&concrete_path).unwrap();

        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        tokio::spawn(async move { while socket.next().await.is_some() {} });

        // No dynamic registration here: the handlers answer regardless of how
        // the capability was announced, and advertising it would make
        // `initialized` block on a registration this drainer never answers.
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }]
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification("initialized", json!({})))
            .await
            .unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": concrete_uri,
                        "languageId": "eventb",
                        "version": 1,
                        "text": CONCRETE,
                    }
                }),
            ))
            .await
            .unwrap();

        macro_rules! ask {
            ($id:expr, $method:expr, $params:expr) => {{
                let request = Request::build($method).id($id).params($params).finish();
                let response = service
                    .ready()
                    .await
                    .unwrap()
                    .call(request)
                    .await
                    .unwrap()
                    .expect("the request must produce a response");
                let (_id, result) = response.into_parts();
                result.expect("the request must succeed")
            }};
        }

        fn names(value: &Value) -> Vec<&str> {
            value
                .as_array()
                .expect("an array of hierarchy items")
                .iter()
                .map(|row| row["name"].as_str().unwrap())
                .collect()
        }

        // Prepare from inside the machine body, not on its name: a user asking
        // while reading an invariant means the enclosing machine.
        let prepared: Value = ask!(
            2,
            "textDocument/prepareTypeHierarchy",
            json!({
                "textDocument": { "uri": concrete_uri },
                "position": { "line": 6, "character": 12 },
            })
        );
        assert_eq!(names(&prepared), ["refined"], "got {prepared}");
        assert_eq!(prepared[0]["detail"], json!("Machine"));
        let refined = prepared[0].clone();

        let supertypes: Value = ask!(3, "typeHierarchy/supertypes", json!({ "item": refined }));
        assert_eq!(
            names(&supertypes),
            ["base"],
            "refined REFINES base; got {supertypes}"
        );
        let base = supertypes[0].clone();

        let subtypes: Value = ask!(4, "typeHierarchy/subtypes", json!({ "item": base.clone() }));
        assert_eq!(
            names(&subtypes),
            ["refined"],
            "base is refined by refined; got {subtypes}"
        );

        // The abstract machine refines nothing, so the tree ends there.
        let top: Value = ask!(5, "typeHierarchy/supertypes", json!({ "item": base }));
        assert!(names(&top).is_empty(), "base refines nothing; got {top}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn implementation_jumps_from_an_abstract_event_to_the_ones_refining_it() {
        let workspace = TempWorkspace::new("type-hierarchy-implementation");
        let abstract_path = workspace.as_ref().join("base.eventb");
        std::fs::write(&abstract_path, ABSTRACT).unwrap();
        std::fs::write(workspace.as_ref().join("refined.eventb"), CONCRETE).unwrap();
        let root_uri = Uri::from_file_path(workspace.as_ref()).unwrap();
        let abstract_uri = Uri::from_file_path(&abstract_path).unwrap();

        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        tokio::spawn(async move { while socket.next().await.is_some() {} });

        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }]
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification("initialized", json!({})))
            .await
            .unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": abstract_uri,
                        "languageId": "eventb",
                        "version": 1,
                        "text": ABSTRACT,
                    }
                }),
            ))
            .await
            .unwrap();

        let implementation = |id: i64, line: u32, character: u32| {
            Request::build("textDocument/implementation")
                .id(id)
                .params(json!({
                    "textDocument": { "uri": abstract_uri },
                    "position": { "line": line, "character": character },
                }))
                .finish()
        };

        // Line 6 is `    EVENT INITIALISATION`; the name starts at column 10.
        // `refined` declares no REFINES on its INITIALISATION, so it refines
        // the same-named abstract event implicitly, the way Rodin reads it.
        let response = service
            .ready()
            .await
            .unwrap()
            .call(implementation(2, 6, 12))
            .await
            .unwrap()
            .expect("implementation must respond");
        let (_id, result) = response.into_parts();
        let value: Value = result.expect("implementation must succeed");
        let rows = value.as_array().expect("an array of locations");
        assert_eq!(rows.len(), 1, "one refining event; got {value}");
        assert!(
            rows[0]["uri"].as_str().unwrap().ends_with("refined.eventb"),
            "the jump lands in the refining machine; got {value}"
        );
        assert_eq!(
            rows[0]["range"]["start"]["line"],
            json!(8),
            "and on its INITIALISATION event name; got {value}"
        );

        // From the machine header instead, the answer is the refining machine.
        let response = service
            .ready()
            .await
            .unwrap()
            .call(implementation(3, 0, 9))
            .await
            .unwrap()
            .unwrap();
        let (_id, result) = response.into_parts();
        let value: Value = result.unwrap();
        let rows = value.as_array().unwrap();
        assert_eq!(rows.len(), 1, "one refining machine; got {value}");
        assert!(
            rows[0]["uri"].as_str().unwrap().ends_with("refined.eventb"),
            "got {value}"
        );
        assert_eq!(
            rows[0]["range"]["start"]["line"],
            json!(0),
            "on the MACHINE header line; got {value}"
        );
    }
}

mod pull_diagnostics {
    //! Wire-level tests for `textDocument/diagnostic` and
    //! `workspace/diagnostic`: the capability is offered only to a client
    //! that declares diagnostic refresh support, an open buffer is left to
    //! the push path so no finding reaches the client twice, the workspace
    //! sweep reaches a file that was never opened, and a closed file's report
    //! is reused until one of its inputs changes.

    use super::{TempWorkspace, next_message, notification};
    use eventb_lsp::lsp_types::Uri;
    use eventb_lsp::server::RossiLanguageServer;
    use futures::StreamExt;
    use serde_json::{Value, json};
    use std::path::Path;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    /// A context whose `CONSTANS` typo the parser reports.
    const BROKEN: &str = "CONTEXT broken\nCONSTANS\n    c\nEND\n";
    const CLEAN: &str =
        "CONTEXT clean\nCONSTANTS\n    c\nAXIOMS\n    @axm1 c \u{2208} \u{2115}\nEND\n";

    /// The client capabilities pull diagnostics are offered to.
    fn refreshing_client() -> Value {
        json!({ "workspace": { "diagnostics": { "refreshSupport": true } } })
    }

    /// The `diagnosticProvider` the server advertises to a client declaring
    /// `capabilities`, `null` when it advertises none.
    async fn advertised(capabilities: Value) -> Value {
        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        tokio::spawn(async move { while socket.next().await.is_some() {} });
        let init = Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": capabilities }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(init)
            .await
            .unwrap()
            .unwrap();
        let (_id, result) = response.into_parts();
        result.unwrap()["capabilities"]["diagnosticProvider"].clone()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pull_is_offered_only_to_clients_that_declare_refresh_support() {
        // Neovim 0.11 and lsp-mode declare none; lsp-mode keeps pulled and
        // pushed findings in one slot, where an empty pull would erase them.
        assert_eq!(advertised(json!({})).await, Value::Null);

        let provider = advertised(refreshing_client()).await;
        assert_eq!(provider["identifier"], json!("rossi"));
        assert_eq!(
            provider["interFileDependencies"],
            json!(true),
            "a SEES / REFINES / EXTENDS edit changes what dependents report"
        );
        assert_eq!(provider["workspaceDiagnostics"], json!(true));
    }

    /// A server initialized on `root` for a client that declares diagnostic
    /// refresh support. Its one-message socket is pumped into an unbounded
    /// receiver, so a later notification never blocks the server.
    async fn initialized_service(
        root: &Path,
    ) -> (
        LspService<RossiLanguageServer>,
        futures::channel::mpsc::UnboundedReceiver<Request>,
    ) {
        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        let (sender, messages) = futures::channel::mpsc::unbounded();
        tokio::spawn(async move {
            while let Some(request) = socket.next().await {
                let _ = sender.unbounded_send(request);
            }
        });
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": refreshing_client(),
                "workspaceFolders": [{ "uri": Uri::from_file_path(root).unwrap(), "name": "test" }]
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification("initialized", json!({})))
            .await
            .unwrap();
        (service, messages)
    }

    /// The findings a full `workspace/diagnostic` sweep reports for `uri`.
    async fn swept(service: &mut LspService<RossiLanguageServer>, id: i64, uri: &Uri) -> Value {
        let request = Request::build("workspace/diagnostic")
            .id(id)
            .params(json!({ "previousResultIds": [] }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap()
            .expect("workspace/diagnostic must respond");
        let (_id, result) = response.into_parts();
        let report: Value = result.expect("workspace/diagnostic must succeed");
        report["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["uri"] == json!(uri))
            .unwrap_or_else(|| panic!("{} must be swept; got {report}", uri.as_str()))["items"]
            .clone()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_open_document_is_left_to_push() {
        let workspace = TempWorkspace::new("pull-diagnostics-doc");
        let file_uri = Uri::from_file_path(workspace.as_ref().join("broken.eventb")).unwrap();
        let (mut service, mut messages) = initialized_service(workspace.as_ref()).await;
        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": file_uri,
                        "languageId": "eventb",
                        "version": 1,
                        "text": BROKEN,
                    }
                }),
            ))
            .await
            .unwrap();

        // The push carries the finding.
        let published = next_message(
            &mut messages,
            "textDocument/publishDiagnostics",
            std::time::Duration::from_secs(5),
        )
        .await
        .expect("opening must publish");
        assert_eq!(published["uri"], json!(file_uri));
        assert!(
            !published["diagnostics"].as_array().unwrap().is_empty(),
            "the CONSTANS typo must be pushed; got {published}"
        );

        // The pull does not repeat it: a client showing both would list every
        // finding twice.
        let request = Request::build("textDocument/diagnostic")
            .id(2)
            .params(json!({ "textDocument": { "uri": file_uri } }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap()
            .expect("textDocument/diagnostic must respond");
        let (_id, result) = response.into_parts();
        let pulled: Value = result.expect("textDocument/diagnostic must succeed");
        assert_eq!(pulled["kind"], json!("full"));
        assert_eq!(
            pulled["items"],
            json!([]),
            "an open document's findings are pushed, not pulled"
        );

        // The workspace sweep lists the open document empty too, so a client
        // drops whatever it pulled while the file was closed.
        assert_eq!(
            swept(&mut service, 3, &file_uri).await,
            json!([]),
            "an open document's findings are pushed, not pulled"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_workspace_sweep_reaches_a_file_that_was_never_opened() {
        let workspace = TempWorkspace::new("pull-diagnostics-workspace");
        let broken = workspace.as_ref().join("broken.eventb");
        let clean = workspace.as_ref().join("clean.eventb");
        std::fs::write(&broken, BROKEN).unwrap();
        std::fs::write(&clean, CLEAN).unwrap();
        let (mut service, _messages) = initialized_service(workspace.as_ref()).await;

        let broken_items = swept(&mut service, 2, &Uri::from_file_path(&broken).unwrap()).await;
        assert!(
            !broken_items.as_array().unwrap().is_empty(),
            "a file nobody opened must still report its parse error; got {broken_items}"
        );

        // The clean sibling is swept too, and reports nothing.
        let clean_items = swept(&mut service, 3, &Uri::from_file_path(&clean).unwrap()).await;
        assert_eq!(clean_items, json!([]), "the clean sibling reports nothing");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_closed_files_report_is_reused_until_one_of_its_inputs_changes() {
        const CONTEXT: &str =
            "CONTEXT C\nCONSTANTS\n    k\nAXIOMS\n    @axm1 k \u{2208} \u{2115}\nEND\n";
        const MACHINE: &str = concat!(
            "MACHINE M\n",
            "SEES C\n",
            "VARIABLES\n",
            "    x\n",
            "INVARIANTS\n",
            "    @inv1 x = k\n",
            "EVENTS\n",
            "    EVENT INITIALISATION\n",
            "    THEN\n",
            "        @act1 x \u{2254} k\n",
            "    END\n",
            "END\n",
        );
        let workspace = TempWorkspace::new("pull-diagnostics-reuse");
        let context = workspace.as_ref().join("C.eventb");
        let machine = workspace.as_ref().join("M.eventb");
        std::fs::write(&context, CONTEXT).unwrap();
        std::fs::write(&machine, MACHINE).unwrap();
        let context_uri = Uri::from_file_path(&context).unwrap();
        let machine_uri = Uri::from_file_path(&machine).unwrap();
        let (mut service, _messages) = initialized_service(workspace.as_ref()).await;

        let first = swept(&mut service, 2, &machine_uri).await;
        assert_eq!(first, json!([]), "M is well formed while C declares k");

        // C loses `k` on disk, and nothing tells the server: M's report is
        // the one computed before, not a fresh check of every closed file.
        std::fs::write(&context, CONTEXT.replace('k', "j")).unwrap();
        let reused = swept(&mut service, 3, &machine_uri).await;
        assert_eq!(reused, first, "an unchanged input set reuses the report");

        // The watcher's notification is the change the server acts on.
        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "workspace/didChangeWatchedFiles",
                json!({ "changes": [{ "uri": context_uri, "type": 2 }] }),
            ))
            .await
            .unwrap();
        let rechecked = swept(&mut service, 4, &machine_uri).await;
        assert_ne!(rechecked, json!([]), "M now uses an undeclared k");

        // A write to the file itself shows through its metadata alone.
        std::fs::write(&machine, MACHINE.replace("VARIABLES", "VARIABLE")).unwrap();
        let rewritten = swept(&mut service, 5, &machine_uri).await;
        assert_ne!(
            rewritten, rechecked,
            "M's own edit is picked up; got {rewritten}"
        );
    }
}

mod proof_obligations {
    //! Wire-level tests for the proof obligation surface: opening a document
    //! pushes `$/rossi/proofStatus`, `rossi/proofObligations` lists the
    //! generated obligations anchored on their source elements together
    //! with the blocks (clauses and events) they sit in, open obligations
    //! show as hints grouped per element, and the lens counts them.

    use super::{next_message, notification};
    use eventb_lsp::server::RossiLanguageServer;
    use serde_json::{Value, json};
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    const URI: &str = "file:///proof.eventb";
    const SOURCE: &str = concat!(
        "MACHINE m\n",
        "VARIABLES\n",
        "    x y\n",
        "INVARIANTS\n",
        "    @inv1 x \u{2208} \u{2115}\n",
        "    @inv2 y \u{2208} \u{2115}\n",
        "EVENTS\n",
        "    EVENT INITIALISATION\n",
        "    THEN\n",
        "        @act1 x \u{2254} 0\n",
        "        @act2 y \u{2254} 0\n",
        "    END\n",
        "\n",
        "    EVENT bump\n",
        "    WHERE\n",
        "        @grd1 x \u{2208} \u{2115}\n",
        "    THEN\n",
        "        @act1 x \u{2254} x + 1\n",
        "    END\n",
        "END\n",
    );

    /// The three obligations `rossi build` generates for `SOURCE`, in the
    /// generator's order.
    const EXPECTED: [&str; 3] = [
        "INITIALISATION/inv1/INV",
        "INITIALISATION/inv2/INV",
        "bump/inv1/INV",
    ];

    /// A server initialised with `options` and `SOURCE` open.
    async fn open_service(
        options: Value,
    ) -> (
        LspService<RossiLanguageServer>,
        tower_lsp_server::ClientSocket,
    ) {
        let (mut service, socket) = LspService::build(RossiLanguageServer::new)
            .custom_method(
                eventb_lsp::proof::REQUEST_OBLIGATIONS,
                RossiLanguageServer::proof_obligations,
            )
            .finish();
        let init = Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {}, "initializationOptions": options }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": URI,
                        "languageId": "eventb",
                        "version": 1,
                        "text": SOURCE,
                    }
                }),
            ))
            .await
            .unwrap();
        (service, socket)
    }

    fn names(rows: &Value) -> Vec<&str> {
        rows.as_array()
            .expect("an array of named rows")
            .iter()
            .map(|o| o["name"].as_str().unwrap())
            .collect()
    }

    /// The blocks of `SOURCE`: the invariants clause, then the events, as
    /// `(name, header line, first line, last line)`.
    const BLOCKS: [(&str, u64, u64, u64); 3] = [
        ("INVARIANTS", 3, 3, 5),
        ("INITIALISATION", 7, 7, 11),
        ("bump", 13, 13, 18),
    ];

    fn assert_blocks(blocks: &Value) {
        let rows = blocks.as_array().expect("an array of blocks");
        let summary: Vec<(&str, u64, u64, u64)> = rows
            .iter()
            .map(|b| {
                (
                    b["name"].as_str().unwrap(),
                    b["header"]["start"]["line"].as_u64().unwrap(),
                    b["range"]["start"]["line"].as_u64().unwrap(),
                    b["range"]["end"]["line"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(summary, BLOCKS, "got {blocks}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn opening_pushes_the_obligation_list() {
        let (_service, mut messages) = open_service(json!({})).await;
        let params = next_message(
            &mut messages,
            eventb_lsp::proof::NOTIFICATION_STATUS,
            std::time::Duration::from_secs(10),
        )
        .await
        .expect("opening a document must push its obligations");
        assert_eq!(params["uri"], json!(URI));
        assert_eq!(names(&params["obligations"]), EXPECTED, "got {params}");
        assert!(
            params["obligations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|o| o["status"] == json!("unattempted")),
            "nothing is proved without a stored proof; got {params}"
        );
        assert_blocks(&params["blocks"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_request_lists_obligations_anchored_on_their_elements() {
        let (mut service, mut messages) = open_service(json!({})).await;
        // Let the open-time refresh land first so the request is served
        // from the overlay rather than recomputing.
        next_message(
            &mut messages,
            eventb_lsp::proof::NOTIFICATION_STATUS,
            std::time::Duration::from_secs(10),
        )
        .await
        .expect("the open-time push");

        let request = Request::build(eventb_lsp::proof::REQUEST_OBLIGATIONS)
            .id(2)
            .params(json!({ "textDocument": { "uri": URI } }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap()
            .expect("rossi/proofObligations must respond");
        let (_id, result) = response.into_parts();
        let value: Value = result.expect("rossi/proofObligations must succeed");
        assert_eq!(names(&value["obligations"]), EXPECTED, "got {value}");
        assert_blocks(&value["blocks"]);
        // The header is the event name or the clause keyword, not the line.
        assert_eq!(
            value["blocks"][1]["header"],
            json!({ "start": { "line": 7, "character": 10 }, "end": { "line": 7, "character": 24 } }),
            "got {value}"
        );
        assert_eq!(value["blocks"][0]["header"]["end"]["character"], json!(10));

        let rows = value["obligations"].as_array().unwrap();
        // `evt/inv1/INV` anchors on the machine's `@inv1` line (4), not on
        // the event: it is the invariant that must be preserved.
        assert_eq!(rows[0]["range"]["start"]["line"], json!(4), "got {value}");
        assert_eq!(rows[1]["range"]["start"]["line"], json!(5), "got {value}");
        assert_eq!(rows[2]["range"]["start"]["line"], json!(4), "got {value}");
        assert_eq!(rows[0]["component"], json!("m"));
        assert_eq!(rows[2]["description"], json!("Invariant  preservation"));
        assert_eq!(rows[2]["accurate"], json!(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_obligations_are_hints_grouped_per_element_and_counted_by_a_lens() {
        let (mut service, mut messages) = open_service(json!({})).await;

        // One publish per open, hints included; the list itself is pushed
        // after it.
        let params = next_message(
            &mut messages,
            "textDocument/publishDiagnostics",
            std::time::Duration::from_secs(10),
        )
        .await
        .expect("opening publishes diagnostics");
        let hints: Vec<Value> = params["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["severity"] == json!(4))
            .cloned()
            .collect();
        next_message(
            &mut messages,
            eventb_lsp::proof::NOTIFICATION_STATUS,
            std::time::Duration::from_secs(10),
        )
        .await
        .expect("the open-time push");
        assert_eq!(hints.len(), 2, "one hint per invariant; got {hints:?}");
        // By default a hint underlines the `@inv1` token, not the element.
        assert_eq!(
            hints[0]["range"],
            json!({ "start": { "line": 4, "character": 4 }, "end": { "line": 4, "character": 9 } })
        );
        assert_eq!(
            hints[0]["message"],
            json!("2 open proof obligations: INITIALISATION/inv1/INV, bump/inv1/INV")
        );
        assert_eq!(hints[1]["range"]["start"]["line"], json!(5));
        assert_eq!(hints[1]["range"]["end"]["character"], json!(9));
        assert_eq!(
            hints[1]["message"],
            json!("1 open proof obligation: INITIALISATION/inv2/INV")
        );

        let request = Request::build("textDocument/codeLens")
            .id(2)
            .params(json!({ "textDocument": { "uri": URI } }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap()
            .unwrap();
        let (_id, result) = response.into_parts();
        let lenses: Value = result.unwrap();
        assert!(
            lenses.as_array().unwrap().iter().any(|lens| {
                lens["command"]["title"] == json!("0/3 proof obligations discharged")
            }),
            "the lens must count the obligations; got {lenses}"
        );
    }

    /// The severity-4 rows of the open-time publish, after letting the
    /// push through so the socket is not left holding it.
    async fn open_hints(options: Value) -> Vec<Value> {
        let (_service, mut messages) = open_service(options).await;
        let params = next_message(
            &mut messages,
            "textDocument/publishDiagnostics",
            std::time::Duration::from_secs(10),
        )
        .await
        .expect("opening publishes diagnostics");
        next_message(
            &mut messages,
            eventb_lsp::proof::NOTIFICATION_STATUS,
            std::time::Duration::from_secs(10),
        )
        .await
        .expect("the open-time push");
        params["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["severity"] == json!(4))
            .cloned()
            .collect()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn element_diagnostics_underline_the_whole_element() {
        let hints = open_hints(json!({ "proofObligations": { "diagnostics": "elements" } })).await;
        assert_eq!(hints.len(), 2, "got {hints:?}");
        // The element's own span, which stops at its last character rather
        // than running into the next line's indentation.
        assert_eq!(
            hints[0]["range"],
            json!({ "start": { "line": 4, "character": 4 }, "end": { "line": 4, "character": 15 } })
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn proof_diagnostics_can_be_turned_off() {
        let hints = open_hints(json!({ "proofObligations": { "diagnostics": "off" } })).await;
        assert!(hints.is_empty(), "got {hints:?}");
    }

    /// Rodin's proof of `INITIALISATION/inv1/INV` in `SOURCE`, whose goal is
    /// `0 ∈ ℕ`: simplification rewrites, then ⊤ goal.
    const PROOF: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.prFile version="1">
<org.eventb.core.prProof name="INITIALISATION/inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.prFresh="" org.eventb.core.prGoal="p0" org.eventb.core.prHyps="">
<org.eventb.core.prRule name="r0" org.eventb.core.confidence="1000" org.eventb.core.prDisplay="simplification rewrites" org.eventb.core.prGoal="p0" org.eventb.core.prHyps="">
<org.eventb.core.prAnte name="'" org.eventb.core.prGoal="p1">
<org.eventb.core.prRule name="r1" org.eventb.core.confidence="1000" org.eventb.core.prDisplay="⊤ goal" org.eventb.core.prGoal="p1" org.eventb.core.prHyps=""/>
</org.eventb.core.prAnte>
</org.eventb.core.prRule>
<org.eventb.core.prPred name="p0" org.eventb.core.predicate="0∈ℕ"/>
<org.eventb.core.prPred name="p1" org.eventb.core.predicate="⊤"/>
<org.eventb.core.prReas name="r0" org.eventb.core.prRID="org.eventb.core.seqprover.autoRewritesL3:2"/>
<org.eventb.core.prReas name="r1" org.eventb.core.prRID="org.eventb.core.seqprover.trueGoal"/>
</org.eventb.core.prProof>
</org.eventb.core.prFile>
"#;

    #[tokio::test(flavor = "current_thread")]
    async fn a_proof_saved_by_rodin_updates_the_count() {
        use super::TempWorkspace;
        use eventb_lsp::lsp_types::Uri;
        use futures::StreamExt;

        let workspace = TempWorkspace::new("proof-saved-by-rodin");
        let root = workspace.as_ref();
        let source = root.join("m.eventb");
        std::fs::write(&source, SOURCE).unwrap();
        // The Rodin workspace exists before the server starts, so its
        // watcher starts with it.
        let project_dir = rossi_build::workspace::default_workspace_dir(root)
            .join(rossi_build::workspace::project_name_for(root, Some(root)));
        std::fs::create_dir_all(&project_dir).unwrap();
        let uri = Uri::from_file_path(&source).unwrap();

        // The socket holds one message, so it is drained into an unbounded
        // channel: `initialized` and the watcher send while the test is not
        // reading.
        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        let (sender, mut messages) = futures::channel::mpsc::unbounded();
        tokio::spawn(async move {
            while let Some(request) = socket.next().await {
                let _ = sender.unbounded_send(request);
            }
        });
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": { "workspace": { "codeLens": { "refreshSupport": true } } },
                "workspaceFolders": [{ "uri": Uri::from_file_path(root).unwrap(), "name": "test" }]
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        for (method, params) in [
            ("initialized", json!({})),
            (
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "eventb",
                        "version": 1,
                        "text": SOURCE,
                    }
                }),
            ),
        ] {
            service
                .ready()
                .await
                .unwrap()
                .call(notification(method, params))
                .await
                .unwrap();
        }
        let opened = next_message(
            &mut messages,
            eventb_lsp::proof::NOTIFICATION_STATUS,
            std::time::Duration::from_secs(10),
        )
        .await
        .expect("the open-time push");
        assert!(
            opened["obligations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|o| o["status"] == json!("unattempted")),
            "nothing is proved before Rodin saves; got {opened}"
        );

        // Rodin saves the proof. The watcher starts on its own thread with
        // no signal when it is up, so the save is repeated until one is seen.
        let mut pushed = None;
        for _ in 0..10 {
            std::fs::write(project_dir.join("m.bpr"), PROOF).unwrap();
            pushed = next_message(
                &mut messages,
                eventb_lsp::proof::NOTIFICATION_STATUS,
                std::time::Duration::from_secs(2),
            )
            .await;
            if pushed.is_some() {
                break;
            }
        }
        let pushed = pushed.expect("a proof Rodin saves must be pushed");
        let statuses: Vec<(&str, &str)> = pushed["obligations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| (o["name"].as_str().unwrap(), o["status"].as_str().unwrap()))
            .collect();
        assert_eq!(
            statuses,
            [
                ("INITIALISATION/inv1/INV", "discharged"),
                ("INITIALISATION/inv2/INV", "unattempted"),
                ("bump/inv1/INV", "unattempted"),
            ],
            "got {pushed}"
        );

        // The editor is asked to fetch its lenses again, and the count they
        // show now includes the proof.
        let refresh = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(request) = messages.next().await {
                if request.method() == "workspace/codeLens/refresh" {
                    return true;
                }
            }
            false
        })
        .await;
        assert_eq!(refresh, Ok(true), "the lenses must be refreshed");
        let request = Request::build("textDocument/codeLens")
            .id(2)
            .params(json!({ "textDocument": { "uri": uri } }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap()
            .unwrap();
        let (_id, result) = response.into_parts();
        let lenses: Value = result.unwrap();
        assert!(
            lenses.as_array().unwrap().iter().any(|lens| {
                lens["command"]["title"] == json!("1/3 proof obligations discharged")
            }),
            "the lens must count the proof; got {lenses}"
        );
    }

    /// The status of the first obligation a `rossi/proofObligations`
    /// request with `params` reports.
    async fn first_status(
        service: &mut LspService<RossiLanguageServer>,
        id: i64,
        params: Value,
    ) -> Value {
        let request = Request::build(eventb_lsp::proof::REQUEST_OBLIGATIONS)
            .id(id)
            .params(params)
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap()
            .expect("rossi/proofObligations must respond");
        let (_id, result) = response.into_parts();
        let report: Value = result.expect("rossi/proofObligations must succeed");
        report["obligations"][0]["status"].clone()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_refresh_request_rereads_the_stored_proofs() {
        use super::TempWorkspace;
        use eventb_lsp::lsp_types::Uri;

        let workspace = TempWorkspace::new("proof-refresh-request");
        let source = workspace.as_ref().join("m.eventb");
        std::fs::write(&source, SOURCE).unwrap();
        let uri = Uri::from_file_path(&source).unwrap();

        let (mut service, mut messages) = LspService::build(RossiLanguageServer::new)
            .custom_method(
                eventb_lsp::proof::REQUEST_OBLIGATIONS,
                RossiLanguageServer::proof_obligations,
            )
            .finish();
        let init = Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {} }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        service
            .ready()
            .await
            .unwrap()
            .call(notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "eventb",
                        "version": 1,
                        "text": SOURCE,
                    }
                }),
            ))
            .await
            .unwrap();
        next_message(
            &mut messages,
            eventb_lsp::proof::NOTIFICATION_STATUS,
            std::time::Duration::from_secs(10),
        )
        .await
        .expect("the open-time push");

        // A proof lands next to the source, with nothing watching it.
        std::fs::write(workspace.as_ref().join("m.bpr"), PROOF).unwrap();

        assert_eq!(
            first_status(&mut service, 2, json!({ "textDocument": { "uri": uri } })).await,
            json!("unattempted"),
            "a plain request is served from the stored list"
        );
        assert_eq!(
            first_status(
                &mut service,
                3,
                json!({ "textDocument": { "uri": uri }, "refresh": true })
            )
            .await,
            json!("discharged"),
            "a refresh request judges the proofs on disk again"
        );
    }
}

mod document_highlight {
    //! Wire-level test for `textDocument/documentHighlight`: the capability is
    //! advertised, every occurrence of the symbol under the cursor comes back
    //! for the requested document, and assignment targets are distinguished
    //! from reads.

    use eventb_lsp::server::RossiLanguageServer;
    use serde_json::{Value, json};
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::Request;

    const SOURCE: &str = concat!(
        "MACHINE m\n",
        "VARIABLES\n",
        "    x y\n",
        "INVARIANTS\n",
        "    @inv1 x \u{2208} \u{2115}\n",
        "    @inv2 y \u{2208} \u{2115}\n",
        "EVENTS\n",
        "    EVENT INITIALISATION\n",
        "    THEN\n",
        "        @act1 x \u{2254} 0\n",
        "        @act2 y \u{2254} 0\n",
        "    END\n",
        "\n",
        "    EVENT bump\n",
        "    WHERE\n",
        "        @grd1 x \u{2208} \u{2115}\n",
        "    THEN\n",
        "        @act1 x \u{2254} x + 1\n",
        "    END\n",
        "END\n",
    );

    async fn highlights_at(line: u32, character: u32) -> Value {
        let (mut service, _socket) = LspService::build(RossiLanguageServer::new).finish();

        let init = Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {} }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(init)
            .await
            .unwrap()
            .expect("initialize must respond");
        let (_id, result) = response.into_parts();
        let capabilities = result.unwrap();
        assert_eq!(
            capabilities["capabilities"]["documentHighlightProvider"],
            json!(true),
            "the server must advertise documentHighlightProvider"
        );

        let open = Request::build("textDocument/didOpen")
            .params(json!({
                "textDocument": {
                    "uri": "file:///highlight.eventb",
                    "languageId": "eventb",
                    "version": 1,
                    "text": SOURCE,
                }
            }))
            .finish();
        service.ready().await.unwrap().call(open).await.unwrap();

        let request = Request::build("textDocument/documentHighlight")
            .id(2)
            .params(json!({
                "textDocument": { "uri": "file:///highlight.eventb" },
                "position": { "line": line, "character": character },
            }))
            .finish();
        let response = service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap()
            .expect("documentHighlight must respond");
        let (_id, result) = response.into_parts();
        result.expect("documentHighlight must succeed")
    }

    /// LSP DocumentHighlightKind: 2 is Read, 3 is Write.
    const READ: i64 = 2;
    const WRITE: i64 = 3;

    #[tokio::test(flavor = "current_thread")]
    async fn highlights_every_occurrence_of_the_symbol_under_the_cursor() {
        // Cursor on `x` in the VARIABLES list (line 2).
        let value = highlights_at(2, 4).await;
        let rows = value.as_array().expect("highlights are a JSON array");

        // Declaration, inv1, act1 of INITIALISATION, grd1, and both sides of
        // `x := x + 1`: six in all, and none of them `y`.
        assert_eq!(rows.len(), 6, "expected six occurrences of x; got {value}");

        let lines: Vec<i64> = rows
            .iter()
            .map(|row| row["range"]["start"]["line"].as_i64().unwrap())
            .collect();
        for line in [2, 4, 9, 15, 17] {
            assert!(
                lines.contains(&line),
                "line {line} must carry an occurrence of x; got {value}"
            );
        }
        assert!(
            !lines.contains(&5),
            "inv2 mentions only y and must not be highlighted; got {value}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn assignment_targets_are_writes_and_uses_are_reads() {
        let value = highlights_at(2, 4).await;
        let rows = value.as_array().unwrap();

        let kind_at = |line: i64, character: i64| -> i64 {
            rows.iter()
                .find(|row| {
                    row["range"]["start"]["line"] == json!(line)
                        && row["range"]["start"]["character"] == json!(character)
                })
                .unwrap_or_else(|| panic!("no highlight at {line}:{character} in {value}"))["kind"]
                .as_i64()
                .expect("every highlight carries a kind")
        };

        // `@act1 x \u{2254} x + 1` on line 17: the target writes, the operand reads.
        assert_eq!(kind_at(17, 14), WRITE, "the assignment target is a write");
        assert_eq!(kind_at(17, 18), READ, "the right-hand operand is a read");
        // The VARIABLES entry is a write.
        assert_eq!(kind_at(2, 4), WRITE, "the declaration is a write");
        // `@grd1 x \u{2208} \u{2115}` on line 15 only reads.
        assert_eq!(kind_at(15, 14), READ, "a guard mention is a read");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_cursor_on_no_identifier_highlights_nothing() {
        // Line 0, character 0 is the `MACHINE` keyword, which no component
        // declares as a name.
        let value = highlights_at(0, 0).await;
        assert!(
            value.is_null() || value.as_array().is_some_and(|rows| rows.is_empty()),
            "a keyword must not produce highlights; got {value}"
        );
    }
}

mod watched_files {
    //! Wire-level regressions for `workspace/didChangeWatchedFiles`: the server
    //! registers the `.eventb` watcher itself, and a change made on disk
    //! outside the editor moves the workspace graph an open document's
    //! cross-file diagnostics are checked against.

    use super::{TempWorkspace, next_published_diagnostics, notification};
    use eventb_lsp::lsp_types::Uri;
    use eventb_lsp::server::RossiLanguageServer;
    use futures::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use std::path::Path;
    use tower::{Service, ServiceExt};
    use tower_lsp_server::LspService;
    use tower_lsp_server::jsonrpc::{Request, Response};

    /// A machine whose `SEES` target is missing until a sibling file appears.
    const MACHINE: &str = "MACHINE m\nSEES ctx\nEND\n";
    const CONTEXT: &str = "CONTEXT ctx\nEND\n";

    /// The rule codes of the next published diagnostics batch.
    async fn next_diagnostic_codes(
        messages: &mut (impl StreamExt<Item = Request> + Unpin),
    ) -> Vec<String> {
        next_published_diagnostics(messages)
            .await
            .iter()
            .map(|diagnostic| diagnostic["code"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// A one-file `workspace/didChangeWatchedFiles` payload. `kind` is the
    /// protocol's `FileChangeType` (1 created, 2 changed, 3 deleted).
    fn watched_change(path: &Path, kind: u8) -> Value {
        json!({
            "changes": [{ "uri": Uri::from_file_path(path).unwrap(), "type": kind }]
        })
    }

    /// Send one notification and wait for the server to finish handling it.
    async fn notify(
        service: &mut LspService<RossiLanguageServer>,
        method: &'static str,
        params: Value,
    ) {
        service
            .ready()
            .await
            .unwrap()
            .call(notification(method, params))
            .await
            .unwrap();
    }

    /// Drive `initialize`/`initialized` against a workspace root, with the
    /// client claiming no optional capabilities (so the server registers
    /// nothing and no request needs answering).
    ///
    /// The server-to-client channel buffers a single message, so its socket is
    /// pumped into an unbounded one for the test to read at its own pace —
    /// otherwise the second notification the server sends blocks its sender
    /// forever.
    async fn initialized_service(
        root: &Path,
    ) -> (
        LspService<RossiLanguageServer>,
        futures::channel::mpsc::UnboundedReceiver<Request>,
    ) {
        let root_uri = Uri::from_file_path(root).unwrap();
        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        let (sender, messages) = futures::channel::mpsc::unbounded();
        tokio::spawn(async move {
            while let Some(request) = socket.next().await {
                if sender.unbounded_send(request).is_err() {
                    break;
                }
            }
        });

        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }]
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();
        notify(&mut service, "initialized", json!({})).await;
        (service, messages)
    }

    /// A server on `workspace` with `mch.eventb` written and open. The caller
    /// asserts the first published batch, since that depends on what else it
    /// staged on disk beforehand.
    async fn service_with_open_machine(
        workspace: &Path,
    ) -> (
        LspService<RossiLanguageServer>,
        futures::channel::mpsc::UnboundedReceiver<Request>,
    ) {
        let machine_path = workspace.join("mch.eventb");
        std::fs::write(&machine_path, MACHINE).unwrap();
        let (mut service, messages) = initialized_service(workspace).await;
        notify(
            &mut service,
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": Uri::from_file_path(&machine_path).unwrap(),
                    "languageId": "eventb",
                    "version": 1,
                    "text": MACHINE
                }
            }),
        )
        .await;
        (service, messages)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_server_registers_the_eventb_watcher_itself() {
        let workspace = TempWorkspace::new("watched-files-registration");
        let root_uri = Uri::from_file_path(workspace.as_ref()).unwrap();

        let (mut service, mut socket) = LspService::build(RossiLanguageServer::new).finish();
        let init = Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {
                    "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } }
                },
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }]
            }))
            .finish();
        service.ready().await.unwrap().call(init).await.unwrap();

        // `initialized` waits for the client's answer to
        // `client/registerCapability`, so the socket has to be served while the
        // notification is still in flight, exactly as a real client does.
        let drive_initialized = async {
            service
                .ready()
                .await
                .unwrap()
                .call(notification("initialized", json!({})))
                .await
                .unwrap();
        };
        let answer = async {
            loop {
                let request = socket
                    .next()
                    .await
                    .expect("the server must ask to register a watcher");
                if request.method() != "client/registerCapability" {
                    continue;
                }
                let (_method, id, params) = request.into_parts();
                let registration =
                    &params.expect("a registration must be sent")["registrations"][0];
                assert_eq!(registration["method"], "workspace/didChangeWatchedFiles");
                assert_eq!(
                    registration["registerOptions"]["watchers"][0]["globPattern"],
                    "**/*.eventb"
                );
                socket
                    .send(Response::from_ok(
                        id.expect("a request carries an id"),
                        json!(null),
                    ))
                    .await
                    .unwrap();
                break;
            }
        };
        tokio::join!(drive_initialized, answer);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_sibling_appearing_on_disk_resolves_an_open_machines_reference() {
        let workspace = TempWorkspace::new("watched-files-graph");
        let context_path = workspace.as_ref().join("ctx.eventb");

        let (mut service, mut messages) = service_with_open_machine(workspace.as_ref()).await;
        assert_eq!(next_diagnostic_codes(&mut messages).await, ["EB009"]);

        // The context appears on disk without ever being opened — a
        // `git checkout`, a `rossi import`, a Rodin write.
        std::fs::write(&context_path, CONTEXT).unwrap();
        notify(
            &mut service,
            "workspace/didChangeWatchedFiles",
            watched_change(&context_path, 1),
        )
        .await;
        assert!(next_diagnostic_codes(&mut messages).await.is_empty());

        // ... and vanishes again, as switching back off the branch would take it.
        std::fs::remove_file(&context_path).unwrap();
        notify(
            &mut service,
            "workspace/didChangeWatchedFiles",
            watched_change(&context_path, 3),
        )
        .await;
        assert_eq!(next_diagnostic_codes(&mut messages).await, ["EB009"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_renamed_sibling_survives_a_create_before_delete_batch() {
        let workspace = TempWorkspace::new("watched-files-rename");
        let context_path = workspace.as_ref().join("ctx.eventb");
        std::fs::write(&context_path, CONTEXT).unwrap();
        let renamed_path = workspace.as_ref().join("renamed.eventb");

        let (mut service, mut messages) = service_with_open_machine(workspace.as_ref()).await;
        assert!(next_diagnostic_codes(&mut messages).await.is_empty());

        // Moving the context's file delivers a create and a delete in one
        // batch, and the client does not order them. With the create first,
        // the new file owns `ctx` before the delete of the old one is
        // processed, so the delete must leave the graph alone.
        std::fs::rename(&context_path, &renamed_path).unwrap();
        notify(
            &mut service,
            "workspace/didChangeWatchedFiles",
            json!({
                "changes": [
                    { "uri": Uri::from_file_path(&renamed_path).unwrap(), "type": 1 },
                    { "uri": Uri::from_file_path(&context_path).unwrap(), "type": 3 },
                ]
            }),
        )
        .await;
        assert!(next_diagnostic_codes(&mut messages).await.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn watched_events_under_dot_directories_are_ignored() {
        let workspace = TempWorkspace::new("watched-files-dot-dir");
        // A generated copy inside the Rodin workspace the scan never descends
        // into. Indexing it would define `m` in a second file and flag the open
        // machine as a duplicate (EB019).
        let generated_dir = workspace.as_ref().join(".rossi").join("rodin");
        std::fs::create_dir_all(&generated_dir).unwrap();
        let generated_path = generated_dir.join("mch.eventb");
        std::fs::write(&generated_path, MACHINE).unwrap();
        let context_path = workspace.as_ref().join("ctx.eventb");

        let (mut service, mut messages) = service_with_open_machine(workspace.as_ref()).await;
        assert_eq!(next_diagnostic_codes(&mut messages).await, ["EB009"]);

        notify(
            &mut service,
            "workspace/didChangeWatchedFiles",
            watched_change(&generated_path, 1),
        )
        .await;

        // An ignored event publishes nothing, so prove it landed nowhere by
        // following it with a real one: the batch it triggers must clear the
        // unresolved reference without ever gaining a duplicate.
        std::fs::write(&context_path, CONTEXT).unwrap();
        notify(
            &mut service,
            "workspace/didChangeWatchedFiles",
            watched_change(&context_path, 1),
        )
        .await;
        assert!(next_diagnostic_codes(&mut messages).await.is_empty());
    }
}
