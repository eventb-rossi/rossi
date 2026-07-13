//! Wire-level regression test for `diagnostics.debounceMs`.
//!
//! A burst of `textDocument/didChange` notifications must coalesce into a single
//! `textDocument/publishDiagnostics` for the final version, rather than one
//! publish per keystroke. Driving the real `LspService` exercises the debounced
//! `tokio::spawn` path end to end (a unit test calling the handler would bypass
//! the runtime that runs the deferred analysis). Each edit's task self-skips at
//! wake-up unless its version is still the document's latest, so only the final
//! edit of a burst analyzes.

use eventb_lsp::server::RossiLanguageServer;
use futures::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::{Service, ServiceExt};
use tower_lsp::LspService;
use tower_lsp::jsonrpc::Request;

const DEBOUNCE_MS: u64 = 120;
const URI: &str = "file:///debounce.eventb";

fn notification(method: &'static str, params: Value) -> Request {
    Request::build(method).params(params).finish()
}

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
