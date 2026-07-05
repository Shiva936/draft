#![no_main]
//! Fuzz the MCP adapter JSON-RPC message handler.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(req) = serde_json::from_str::<serde_json::Value>(s) {
            // Handler must never panic and must never run a gated operation.
            let _ = draft_adapters::mcp::handle(std::path::Path::new("."), &req);
        }
    }
});
