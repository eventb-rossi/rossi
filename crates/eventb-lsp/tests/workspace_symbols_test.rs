//! Wire-level regressions for the disk-backed workspace symbol index.

use eventb_lsp::lsp_types::Url;
use eventb_lsp::server::RossiLanguageServer;
use futures::StreamExt;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tower::{Service, ServiceExt};
use tower_lsp::LspService;
use tower_lsp::jsonrpc::Request;

struct TempWorkspace(PathBuf);

impl TempWorkspace {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "workspace-symbols-test-{}-{}",
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

fn notification(method: &'static str, params: Value) -> Request {
    Request::build(method).params(params).finish()
}

#[tokio::test(flavor = "current_thread")]
async fn disk_symbols_are_overlaid_while_open_and_restored_on_close() {
    let workspace = TempWorkspace::new();
    let path = workspace.as_ref().join("model.eventb");
    std::fs::write(
        &path,
        "CONTEXT disk_context\nCONSTANTS\n    disk_value\nEND\n",
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&path, workspace.as_ref().join("alias.eventb")).unwrap();
    let root_uri = Url::from_file_path(workspace.as_ref()).unwrap();
    let file_uri = Url::from_file_path(&path).unwrap();

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
