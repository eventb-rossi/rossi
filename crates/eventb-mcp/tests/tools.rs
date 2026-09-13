//! The verification tools, driven by an in-process client over a duplex
//! pipe, against the bundled example models.

use std::path::{Path, PathBuf};

use rmcp::model::{CallToolRequestParams, ClientInfo};
use rmcp::service::RunningService;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Value, json};

use eventb_mcp::RossiServer;

#[derive(Clone, Default)]
struct Client;

impl ClientHandler for Client {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../rossi/examples")
}

/// A fresh root holding copies of the named example files.
fn root_with(name: &str, files: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eventb-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for file in files {
        std::fs::copy(examples().join(file), dir.join(file)).unwrap();
    }
    dir
}

async fn connect(root: PathBuf) -> RunningService<RoleClient, Client> {
    let (server_transport, client_transport) = tokio::io::duplex(1 << 16);
    tokio::spawn(async move {
        let running = RossiServer::new(root)
            .serve(server_transport)
            .await
            .expect("the server starts");
        let _ = running.waiting().await;
    });
    Client
        .serve(client_transport)
        .await
        .expect("the client connects")
}

/// Call a tool; the flag says whether the result was an error document.
async fn call(
    client: &RunningService<RoleClient, Client>,
    name: &str,
    args: Value,
) -> (bool, Value) {
    let params = match args.as_object() {
        Some(arguments) => {
            CallToolRequestParams::new(name.to_string()).with_arguments(arguments.clone())
        }
        None => CallToolRequestParams::new(name.to_string()),
    };
    let result = client.call_tool(params).await.expect("the call completes");
    let is_error = result.is_error.unwrap_or(false);
    let document = result
        .structured_content
        .expect("every result is a structured document");
    (is_error, document)
}

#[tokio::test]
async fn the_server_lists_its_tools() {
    let client = connect(root_with("tools", &[])).await;
    let mut names: Vec<String> = client
        .list_all_tools()
        .await
        .expect("tools list")
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["build", "project", "validate"]);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn project_validate_and_build_over_an_example() {
    let root = root_with(
        "bank",
        &["bank_account_ctx.eventb", "bank_account_machine.eventb"],
    );
    let client = connect(root.clone()).await;

    let (error, project) = call(&client, "project", Value::Null).await;
    assert!(!error, "{project}");
    assert_eq!(project["kind"], "text");
    let mut components: Vec<(String, String)> = project["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["kind"].as_str().unwrap().into(),
                c["name"].as_str().unwrap().into(),
            )
        })
        .collect();
    components.sort();
    assert_eq!(
        components,
        [
            ("context".to_string(), "bank_account_ctx".to_string()),
            ("machine".to_string(), "bank_account".to_string()),
        ]
    );
    assert_eq!(
        project["edges"],
        json!([{"from": "bank_account", "to": "bank_account_ctx", "kind": "sees"}])
    );
    assert_eq!(project["diagnostics"]["errors"], 0, "{project}");

    let (error, validation) = call(&client, "validate", json!({})).await;
    assert!(!error, "{validation}");
    assert_eq!(validation["counts"]["errors"], 0, "{validation}");

    let (error, build) = call(&client, "build", json!({})).await;
    assert!(!error, "{build}");
    assert_eq!(build["written"], true);
    let files: Vec<&str> = build["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["filename"].as_str().unwrap())
        .collect();
    assert!(files.contains(&"bank_account.bcm"), "{files:?}");
    assert!(files.contains(&"bank_account.bpo"), "{files:?}");
    let total = build["obligations"]["total"].as_u64().unwrap();
    assert!(total > 0, "{build}");
    assert_eq!(build["proofs"]["total"], total, "{build}");
    assert_eq!(build["proofs"]["unattempted"], total, "{build}");
    let output_dir = PathBuf::from(build["output_dir"].as_str().unwrap());
    assert!(output_dir.starts_with(&root), "{output_dir:?}");
    assert!(output_dir.join("bank_account.bpo").is_file());
    assert!(output_dir.join("bank_account.bps").is_file());

    // Nothing changed: the second call reuses the load, and asking not to
    // write leaves the files alone.
    let (error, again) = call(&client, "build", json!({"write": false})).await;
    assert!(!error, "{again}");
    assert_eq!(again["written"], false);
    assert_eq!(again["obligations"]["total"], total);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_file_that_does_not_parse_is_a_located_finding() {
    let root = root_with("broken", &[]);
    std::fs::write(
        root.join("broken.eventb"),
        "MACHINE m\nINVARIANTS\n    @inv1 x ∈\nEND\n",
    )
    .unwrap();
    let client = connect(root).await;

    let (error, validation) = call(&client, "validate", json!({})).await;
    assert!(!error, "{validation}");
    let findings = validation["diagnostics"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{validation}");
    assert_eq!(findings[0]["severity"], "error");
    assert_eq!(findings[0]["file"], "broken.eventb");
    assert!(
        findings[0]["rule_id"] == "EB004" || findings[0]["rule_id"] == "EB005",
        "{validation}"
    );
    assert!(findings[0]["region"].is_object(), "{validation}");

    let (error, build) = call(&client, "build", json!({})).await;
    assert!(error, "{build}");
    assert_eq!(build["error"], "the model does not parse");
    assert_eq!(build["diagnostics"].as_array().unwrap().len(), 1);

    client.cancel().await.unwrap();
}

/// The well-definedness conditions are `info`, below the default floor,
/// so asking for them must lower the floor rather than filter them away.
#[tokio::test]
async fn asking_for_the_well_definedness_conditions_reports_them() {
    let root = root_with("wd", &[]);
    std::fs::write(
        root.join("c.eventb"),
        "CONTEXT c\nCONSTANTS f y\nAXIOMS\n    @a1 f ∈ ℕ ⇸ ℕ\n    @a2 y = f(1)\nEND\n",
    )
    .unwrap();
    let client = connect(root).await;

    let (error, plain) = call(&client, "validate", json!({})).await;
    assert!(!error, "{plain}");
    assert_eq!(plain["diagnostics"].as_array().unwrap().len(), 0, "{plain}");

    let (error, with_wd) = call(&client, "validate", json!({"include_wd": true})).await;
    assert!(!error, "{with_wd}");
    let findings = with_wd["diagnostics"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{with_wd}");
    assert_eq!(findings[0]["rule_id"], "EB010");
    assert_eq!(findings[0]["severity"], "info");
    assert_eq!(findings[0]["element"], "a2");
    assert_eq!(with_wd["counts"]["infos"], 1, "{with_wd}");

    // An explicit floor still wins over the convenience.
    let (_, errors_only) = call(
        &client,
        "validate",
        json!({"include_wd": true, "severity": "error"}),
    )
    .await;
    assert_eq!(errors_only["diagnostics"].as_array().unwrap().len(), 0);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn an_edit_is_picked_up_by_the_next_call() {
    let root = root_with(
        "edit",
        &["bank_account_ctx.eventb", "bank_account_machine.eventb"],
    );
    let client = connect(root.clone()).await;
    let (_, before) = call(&client, "validate", json!({})).await;
    assert_eq!(before["counts"]["errors"], 0, "{before}");

    let ctx = root.join("bank_account_ctx.eventb");
    let text = std::fs::read_to_string(&ctx).unwrap();
    let edited = text.replace(
        "@axm5 min_balance < max_balance",
        "@axm5 min_balance < max_balance\n    @axm6 zz = 1",
    );
    assert_ne!(text, edited, "the example still has axm5");
    std::fs::write(&ctx, edited).unwrap();

    let (error, after) = call(&client, "validate", json!({"severity": "error"})).await;
    assert!(!error, "{after}");
    let findings = after["diagnostics"].as_array().unwrap();
    assert!(!findings.is_empty(), "{after}");
    assert!(findings.iter().all(|f| f["severity"] == "error"), "{after}");
    assert!(
        findings
            .iter()
            .any(|f| f["component"] == "bank_account_ctx" && f["file"] == "bank_account_ctx.eventb"),
        "{after}"
    );

    client.cancel().await.unwrap();
}

/// The tool surface is a contract: names, descriptions, annotations and
/// the input and output schemas the client sees. It is committed so an
/// upgrade of the SDK or of the schema generator shows its effect as a
/// diff. Rerun with `EVENTB_MCP_SCHEMA_REGENERATE=1` to accept a change.
#[tokio::test]
async fn the_tool_schemas_are_the_committed_ones() {
    let client = connect(root_with("schema", &[])).await;
    let mut tools = client.list_all_tools().await.expect("tools list");
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let actual = serde_json::to_string_pretty(&tools).unwrap() + "\n";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tools.json");
    if std::env::var_os("EVENTB_MCP_SCHEMA_REGENERATE").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        actual, expected,
        "the tool schemas changed; review the diff and rerun with \
         EVENTB_MCP_SCHEMA_REGENERATE=1 to accept it"
    );
    client.cancel().await.unwrap();
}
