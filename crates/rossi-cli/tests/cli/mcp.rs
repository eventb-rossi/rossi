//! `rossi mcp`: the server answers the protocol's handshake and lists its
//! tools over stdio.

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;

use crate::helpers::{rossi_command, tempdir_unique};

/// One JSON-RPC message per line, as the stdio transport frames them.
fn message(value: &serde_json::Value) -> String {
    format!("{value}\n")
}

#[test]
fn mcp_serves_the_handshake_and_the_tool_list() {
    let root = tempdir_unique("rossi-cli-mcp");
    std::fs::write(root.join("m.eventb"), "MACHINE m\nEND\n").unwrap();
    let mut child = rossi_command()
        .args(["mcp", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());

    stdin
        .write_all(
            message(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "rossi-cli-test", "version": "0"}
                }
            }))
            .as_bytes(),
        )
        .unwrap();
    stdin
        .write_all(
            message(&serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .as_bytes(),
        )
        .unwrap();
    stdin
        .write_all(
            message(&serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}))
                .as_bytes(),
        )
        .unwrap();
    stdin.flush().unwrap();

    let mut initialized = None;
    let mut tools = None;
    for line in stdout.lines() {
        let line = line.expect("a line of output");
        let reply: serde_json::Value = serde_json::from_str(&line).expect("a JSON-RPC message");
        match reply["id"].as_u64() {
            Some(1) => initialized = Some(reply),
            Some(2) => {
                tools = Some(reply);
                break;
            }
            _ => {}
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    let initialized = initialized.expect("the initialize reply");
    assert_eq!(
        initialized["result"]["serverInfo"]["name"], "rossi",
        "{initialized}"
    );
    assert!(
        initialized["result"]["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("validate")),
        "{initialized}"
    );
    let tools = tools.expect("the tools/list reply");
    let mut names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "build",
            "check_invariants_cbc",
            "check_wd",
            "disprove_po",
            "get_po",
            "list_pos",
            "model_check",
            "project",
            "proof_status",
            "validate",
        ]
    );
}
