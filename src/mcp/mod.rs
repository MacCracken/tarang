//! MCP (Model Context Protocol) server for tarang.
//!
//! Exposes tarang's media analysis capabilities as MCP tools over JSON-RPC stdio.

pub mod tools;

pub use tools::{handle_async_tool_call, handle_tool_call};
// re-exported for tests in main.rs
#[allow(unused_imports)]
pub use tools::{error_response, open_and_probe, require_path, success_response};

use anyhow::Result;
use serde_json::{Value, json};
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Maximum MCP message size in bytes (10 MB).
pub const MAX_MCP_MESSAGE_BYTES: usize = 10_485_760;

pub async fn cmd_mcp() -> Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    // MCP server loop
    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > MAX_MCP_MESSAGE_BYTES {
            tracing::warn!("Rejecting oversized message ({} bytes)", line.len());
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Failed to parse JSON-RPC request: {e}");
                continue;
            }
        };

        let method = request["method"].as_str().unwrap_or("");
        let id = &request["id"];

        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "tarang", "version": env!("CARGO_PKG_VERSION") }
            }),
            "tools/list" => json!({
                "tools": [
                    {
                        "name": "tarang_probe",
                        "description": "Probe a media file and return format, codec, duration, and stream info",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "path": { "type": "string", "description": "Path to media file" } },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "tarang_analyze",
                        "description": "AI-powered media content analysis — classify type, quality, suggest codecs",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "path": { "type": "string", "description": "Path to media file" } },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "tarang_codecs",
                        "description": "List all supported audio and video codecs with their backends",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "tarang_transcribe",
                        "description": "Prepare a transcription request for audio content (routes to hoosh)",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "Path to media file" },
                                "language": { "type": "string", "description": "Language hint (e.g. 'en', 'ja')" }
                            },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "tarang_formats",
                        "description": "Detect media container format from file header magic bytes",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "path": { "type": "string", "description": "Path to media file" } },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "tarang_fingerprint_index",
                        "description": "Compute audio fingerprint and index in the AGNOS vector store for similarity search",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "path": { "type": "string", "description": "Path to audio file" } },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "tarang_search_similar",
                        "description": "Find media files similar to a given file using audio fingerprint matching",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "Path to reference audio file" },
                                "top_k": { "type": "integer", "description": "Number of results (default: 5)" }
                            },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "tarang_describe",
                        "description": "Generate a rich AI content description using LLM analysis via hoosh",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "path": { "type": "string", "description": "Path to media file" } },
                            "required": ["path"]
                        }
                    }
                ]
            }),
            "tools/call" => {
                let tool_name = request["params"]["name"].as_str().unwrap_or("");
                let args = &request["params"]["arguments"];
                match tool_name {
                    "tarang_fingerprint_index" | "tarang_search_similar" | "tarang_describe" => {
                        handle_async_tool_call(tool_name, args).await
                    }
                    _ => handle_tool_call(tool_name, args),
                }
            }
            _ => json!({ "error": format!("unknown method: {method}") }),
        };

        let response = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        });
        {
            let stdout = std::io::stdout();
            let mut writer = std::io::BufWriter::new(stdout.lock());
            serde_json::to_writer(&mut writer, &response)?;
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
        // Exactly at limit should pass
        let at_limit = "x".repeat(MAX_MCP_MESSAGE_BYTES);
        assert!(!is_oversized_message(&at_limit));

        // One byte over limit should be rejected
        let over_limit = "x".repeat(MAX_MCP_MESSAGE_BYTES + 1);
        assert!(is_oversized_message(&over_limit));

        // Empty message should pass
        assert!(!is_oversized_message(""));

        // Normal-sized message should pass
        let normal = r#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#;
        assert!(!is_oversized_message(normal));
    }
}
