//! MCP adapter — JSON-RPC 2.0 over stdio (TDD §41).
//!
//! Exposes only *safe* Draft operations to AI tools. Reads (list/inspect/risk/
//! events) and evidence-producing verification/creation/export are allowed and
//! emit signed receipts through core; **save, rollback, and approve are refused**
//! because they require explicit human approval. No private keys are ever
//! exposed.

use draft_core::App;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The safe tools this adapter advertises.
pub fn tool_list() -> Value {
    json!({
        "tools": [
            tool("draft.pack_list", "List all changepacks in the workspace", json!({})),
            tool("draft.pack_inspect", "Inspect a pack's manifest, state, and impact",
                 json!({"pack_id": {"type": "string"}})),
            tool("draft.pack_create", "Create a changepack from current changes",
                 json!({"name": {"type": "string"}})),
            tool("draft.verify", "Verify a pack (risk + evidence-based selection)",
                 json!({"pack_id": {"type": "string"}, "full": {"type": "boolean"}, "fuzz": {"type": "boolean"}})),
            tool("draft.pack_risk", "Read a pack's risk report",
                 json!({"pack_id": {"type": "string"}})),
            tool("draft.events", "Read the canonical event history", json!({})),
            tool("draft.pack_export", "Export a pack to a portable .draftpack",
                 json!({"pack_id": {"type": "string"}})),
        ]
    })
}

fn tool(name: &str, desc: &str, props: Value) -> Value {
    json!({
        "name": name,
        "description": desc,
        "inputSchema": {"type": "object", "properties": props}
    })
}

/// Operations that are never available over MCP (need human approval).
fn is_gated(name: &str) -> bool {
    matches!(
        name,
        "draft.save" | "draft.rollback" | "draft.approve" | "draft.reject"
    )
}

/// Handle one JSON-RPC request, returning the JSON-RPC response value.
pub fn handle(root: &Path, req: &Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "initialize" => ok(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "draft-mcp", "version": draft_core::DRAFT_VERSION},
                "capabilities": {"tools": {}}
            }),
        ),
        "tools/list" => ok(id, tool_list()),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if is_gated(name) {
                return err(
                    id,
                    -32000,
                    &format!(
                        "'{name}' requires explicit human approval and is not available over MCP"
                    ),
                );
            }
            match call_tool(root, name, &args) {
                Ok(value) => ok(
                    id,
                    json!({"content": [{"type": "text", "text": value.to_string()}], "isError": false}),
                ),
                Err(msg) => ok(
                    id,
                    json!({"content": [{"type": "text", "text": msg}], "isError": true}),
                ),
            }
        }
        other => err(id, -32601, &format!("method not found: {other}")),
    }
}

fn to_val<T: serde::Serialize>(r: draft_core::error::DraftResult<T>) -> Result<Value, String> {
    r.map_err(|e| e.message)
        .and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
}

/// Dispatch a safe tool call to a core operation.
pub fn call_tool(root: &Path, name: &str, args: &Value) -> Result<Value, String> {
    let app = App::new();
    let pack_id = || {
        args.get("pack_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match name {
        "draft.pack_list" => to_val(app.list_canonical_packs(root)),
        "draft.pack_inspect" => to_val(app.pack_inspect(root, &pack_id())),
        "draft.pack_create" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("missing 'name'")?;
            to_val(app.pack_create_from_base(root, name.to_string(), None))
        }
        "draft.verify" => {
            let full = args.get("full").and_then(Value::as_bool).unwrap_or(false);
            let fuzz = args.get("fuzz").and_then(Value::as_bool).unwrap_or(false);
            to_val(app.verify_pack_v2(root, &pack_id(), full, fuzz))
        }
        "draft.pack_risk" => app.pack_risk_json(root, &pack_id()).map_err(|e| e.message),
        "draft.events" => to_val(app.canonical_events(root)),
        "draft.pack_export" => to_val(app.pack_export(root, &pack_id(), None)),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}
fn err(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Serve MCP over stdio: one JSON request per line, one JSON response per line.
pub fn serve_stdio(root: PathBuf) -> Result<(), String> {
    let _ = crate::ensure_adapter_config("mcp");
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(&root, &req),
            Err(e) => err(Value::Null, -32700, &format!("parse error: {e}")),
        };
        writeln!(stdout, "{response}").map_err(|e| e.to_string())?;
        stdout.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_and_tools_list() {
        let init = handle(
            Path::new("."),
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "draft-mcp");
        let tools = handle(
            Path::new("."),
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        );
        let names: Vec<_> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"draft.verify"));
        assert!(!names.iter().any(|n| n.contains("save")));
    }

    #[test]
    fn dangerous_ops_are_gated() {
        let resp = handle(
            Path::new("."),
            &json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
                    "params":{"name":"draft.save","arguments":{}}}),
        );
        assert!(resp["error"]["message"]
            .as_str()
            .unwrap()
            .contains("human approval"));
    }

    #[test]
    fn unknown_method_errors() {
        let resp = handle(
            Path::new("."),
            &json!({"jsonrpc":"2.0","id":4,"method":"nope"}),
        );
        assert_eq!(resp["error"]["code"], -32601);
    }
}
