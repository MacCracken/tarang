//! MCP (Model Context Protocol) server for tarang.
//!
//! Exposes tarang's media analysis capabilities as MCP tools via bote's
//! JSON-RPC dispatch over stdio.

pub mod tools;

pub use tools::{handle_async_tool_call, handle_tool_call};
// re-exported for tests in main.rs
#[allow(unused_imports)]
pub use tools::{error_response, open_and_probe, require_path, success_response};

use anyhow::Result;
use bote::registry::{ToolDef, ToolRegistry, ToolSchema};
use bote::transport;
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Maximum MCP message size in bytes (10 MB).
pub const MAX_MCP_MESSAGE_BYTES: usize = 10_485_760;

/// Build tool definitions for tarang's MCP tools.
#[must_use]
pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef::new(
            "tarang_probe",
            "Probe a media file and return format, codec, duration, and stream info",
            ToolSchema::new(
                "object",
                HashMap::from([(
                    "path".into(),
                    serde_json::json!({"type": "string", "description": "Path to media file"}),
                )]),
                vec!["path".into()],
            ),
        ),
        ToolDef::new(
            "tarang_analyze",
            "AI-powered media content analysis — classify type, quality, suggest codecs",
            ToolSchema::new(
                "object",
                HashMap::from([(
                    "path".into(),
                    serde_json::json!({"type": "string", "description": "Path to media file"}),
                )]),
                vec!["path".into()],
            ),
        ),
        ToolDef::new(
            "tarang_codecs",
            "List all supported audio and video codecs with their backends",
            ToolSchema::new("object", HashMap::new(), vec![]),
        ),
        ToolDef::new(
            "tarang_transcribe",
            "Prepare a transcription request for audio content (routes to hoosh)",
            ToolSchema::new(
                "object",
                HashMap::from([
                    (
                        "path".into(),
                        serde_json::json!({"type": "string", "description": "Path to media file"}),
                    ),
                    (
                        "language".into(),
                        serde_json::json!({"type": "string", "description": "Language hint (e.g. 'en', 'ja')"}),
                    ),
                ]),
                vec!["path".into()],
            ),
        ),
        ToolDef::new(
            "tarang_formats",
            "Detect media container format from file header magic bytes",
            ToolSchema::new(
                "object",
                HashMap::from([(
                    "path".into(),
                    serde_json::json!({"type": "string", "description": "Path to media file"}),
                )]),
                vec!["path".into()],
            ),
        ),
        ToolDef::new(
            "tarang_fingerprint_index",
            "Compute audio fingerprint and index in the AGNOS vector store for similarity search",
            ToolSchema::new(
                "object",
                HashMap::from([(
                    "path".into(),
                    serde_json::json!({"type": "string", "description": "Path to audio file"}),
                )]),
                vec!["path".into()],
            ),
        ),
        ToolDef::new(
            "tarang_search_similar",
            "Find media files similar to a given file using audio fingerprint matching",
            ToolSchema::new(
                "object",
                HashMap::from([
                    (
                        "path".into(),
                        serde_json::json!({"type": "string", "description": "Path to reference audio file"}),
                    ),
                    (
                        "top_k".into(),
                        serde_json::json!({"type": "integer", "description": "Number of results (default: 5)"}),
                    ),
                ]),
                vec!["path".into()],
            ),
        ),
        ToolDef::new(
            "tarang_describe",
            "Generate a rich AI content description using LLM analysis via hoosh",
            ToolSchema::new(
                "object",
                HashMap::from([(
                    "path".into(),
                    serde_json::json!({"type": "string", "description": "Path to media file"}),
                )]),
                vec!["path".into()],
            ),
        ),
    ]
}

/// Build a bote Dispatcher with all tarang tool handlers registered.
fn build_dispatcher() -> bote::Dispatcher {
    let mut reg = ToolRegistry::new();
    for def in tool_defs() {
        reg.register(def);
    }
    let mut dispatcher = bote::Dispatcher::new(reg);

    // Register per-tool sync handlers. Each closure captures its tool name.
    let sync_tools: &[&str] = &[
        "tarang_probe",
        "tarang_analyze",
        "tarang_codecs",
        "tarang_transcribe",
        "tarang_formats",
    ];
    for &name in sync_tools {
        let tool_name = name.to_string();
        dispatcher.handle(
            name,
            Arc::new(move |params| handle_tool_call(&tool_name, &params)),
        );
    }

    // Async tools get placeholder sync handlers — actual async dispatch
    // is handled in cmd_mcp() before reaching the dispatcher.
    for name in &[
        "tarang_fingerprint_index",
        "tarang_search_similar",
        "tarang_describe",
    ] {
        dispatcher.handle(
            *name,
            Arc::new(|_| {
                serde_json::json!({
                    "content": [{"type": "text", "text": "async tool — use streaming dispatch"}],
                    "isError": true
                })
            }),
        );
    }

    dispatcher
}

pub async fn cmd_mcp() -> Result<()> {
    let dispatcher = build_dispatcher();
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > MAX_MCP_MESSAGE_BYTES {
            tracing::warn!("Rejecting oversized message ({} bytes)", line.len());
            continue;
        }

        // Check if it's an async tool call — need special handling since
        // bote's sync Dispatcher can't run async handlers.
        if let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) {
            if request["method"].as_str() == Some("tools/call") {
                let tool_name = request["params"]["name"].as_str().unwrap_or("");
                if matches!(
                    tool_name,
                    "tarang_fingerprint_index" | "tarang_search_similar" | "tarang_describe"
                ) {
                    let id = &request["id"];
                    let args = &request["params"]["arguments"];
                    let result = handle_async_tool_call(tool_name, args).await;
                    let response = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result
                    });
                    let stdout = std::io::stdout();
                    let mut writer = std::io::BufWriter::new(stdout.lock());
                    serde_json::to_writer(&mut writer, &response)?;
                    writeln!(writer)?;
                    writer.flush()?;
                    continue;
                }
            }
        }

        // All other messages: delegate to bote's transport codec + dispatcher.
        if let Some(response_json) = transport::process_message(&line, &dispatcher) {
            let stdout = std::io::stdout();
            let mut writer = std::io::BufWriter::new(stdout.lock());
            write!(writer, "{response_json}")?;
            writeln!(writer)?;
            writer.flush()?;
        }
    }

    Ok(())
}

/// Check whether a message exceeds the MCP size limit. Returns true if rejected.
#[allow(dead_code)]
pub fn is_oversized_message(msg: &str) -> bool {
    msg.len() > MAX_MCP_MESSAGE_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_oversized_message_rejected() {
        let at_limit = "x".repeat(MAX_MCP_MESSAGE_BYTES);
        assert!(!is_oversized_message(&at_limit));

        let over_limit = "x".repeat(MAX_MCP_MESSAGE_BYTES + 1);
        assert!(is_oversized_message(&over_limit));

        assert!(!is_oversized_message(""));

        let normal = r#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
        assert!(!is_oversized_message(normal));
    }

    #[test]
    fn test_tool_defs_count() {
        let defs = tool_defs();
        assert_eq!(defs.len(), 8);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"tarang_probe"));
        assert!(names.contains(&"tarang_describe"));
    }

    #[test]
    fn test_tool_defs_schemas_valid() {
        for def in tool_defs() {
            assert_eq!(def.input_schema.schema_type, "object");
            assert!(!def.description.is_empty());
        }
    }

    #[test]
    fn test_dispatcher_handles_initialize() {
        let dispatcher = build_dispatcher();
        let req = bote::JsonRpcRequest::new(1, "initialize");
        let resp = dispatcher.dispatch(&req).unwrap();
        assert!(resp.result.is_some());
    }

    #[test]
    fn test_dispatcher_lists_tools() {
        let dispatcher = build_dispatcher();
        let req = bote::JsonRpcRequest::new(1, "tools/list");
        let resp = dispatcher.dispatch(&req).unwrap();
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 8);
    }
}
