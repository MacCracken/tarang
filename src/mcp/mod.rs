pub mod tools;

pub use tools::{handle_async_tool_call, handle_tool_call};
// re-exported for tests in main.rs
#[allow(unused_imports)]
pub use tools::{error_response, open_and_probe, require_path, success_response};

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};

pub async fn cmd_mcp() -> Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    // MCP server loop
    while let Ok(Some(line)) = lines.next_line().await {
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
        println!("{}", serde_json::to_string(&response)?);
    }

    Ok(())
}
