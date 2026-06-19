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
use tower_lsp::LspService;
use tower_lsp::jsonrpc::Request;

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
