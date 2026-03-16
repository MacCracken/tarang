//! tarang — AI-native Rust media framework for AGNOS
//!
//! Tarang (Sanskrit: wave) provides media decoding with a Rust-owned pipeline,
//! pure Rust audio decoding via symphonia, and thin FFI wrappers for video codecs.

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tarang",
    version,
    about = "AI-native Rust media framework for AGNOS"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Probe a media file and display info
    Probe {
        /// Path to media file
        path: String,
    },
    /// Analyze media content with AI classification
    Analyze {
        /// Path to media file
        path: String,
    },
    /// List supported codecs
    Codecs,
    /// Run as MCP server on stdio
    Mcp,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Probe { path } => cmd_probe(&path)?,
        Commands::Analyze { path } => cmd_analyze(&path)?,
        Commands::Codecs => cmd_codecs(),
        Commands::Mcp => cmd_mcp().await?,
    }

    Ok(())
}

fn cmd_probe(path: &str) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let info = tarang_audio::probe_audio(file)?;

    println!("Format:   {}", info.format);
    if let Some(d) = info.duration {
        println!("Duration: {:.1}s", d.as_secs_f64());
    }
    println!("Streams:  {}", info.streams.len());

    for (i, stream) in info.streams.iter().enumerate() {
        match stream {
            tarang_core::StreamInfo::Audio(a) => {
                println!(
                    "  [{}] Audio: {} {}Hz {}ch",
                    i, a.codec, a.sample_rate, a.channels
                );
            }
            tarang_core::StreamInfo::Video(v) => {
                println!(
                    "  [{}] Video: {} {}x{} {:.1}fps",
                    i, v.codec, v.width, v.height, v.frame_rate
                );
            }
            tarang_core::StreamInfo::Subtitle { language } => {
                println!(
                    "  [{}] Subtitle: {}",
                    i,
                    language.as_deref().unwrap_or("unknown")
                );
            }
        }
    }

    Ok(())
}

fn cmd_analyze(path: &str) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let info = tarang_audio::probe_audio(file)?;
    let analysis = tarang_ai::analyze_media(&info);

    println!("Content type: {}", analysis.content_type);
    println!("Quality:      {:.0}/100", analysis.quality_score);
    println!("Complexity:   {:.1}", analysis.estimated_complexity);
    println!("Tags:         {}", analysis.tags.join(", "));
    if let Some(rec) = analysis.codec_recommendation {
        println!("Suggestion:   {}", rec);
    }

    Ok(())
}

fn cmd_codecs() {
    println!("Audio codecs (pure Rust via symphonia):");
    for codec in tarang_audio::supported_codecs() {
        println!("  {codec}");
    }
    println!();
    println!("Video codecs (C FFI backends):");
    for (codec, backend) in tarang_video::supported_codecs() {
        println!("  {codec} — {backend}");
    }
}

async fn cmd_mcp() -> Result<()> {
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, BufReader};

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    // MCP server loop
    while let Ok(Some(line)) = lines.next_line().await {
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
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

fn handle_tool_call(name: &str, args: &serde_json::Value) -> serde_json::Value {
    use serde_json::json;

    match name {
        "tarang_probe" => {
            let path = args["path"].as_str().unwrap_or("");
            match std::fs::File::open(path) {
                Ok(file) => match tarang_audio::probe_audio(file) {
                    Ok(info) => json!({
                        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&info).unwrap_or_default() }]
                    }),
                    Err(e) => {
                        json!({ "content": [{ "type": "text", "text": format!("probe error: {e}") }], "isError": true })
                    }
                },
                Err(e) => {
                    json!({ "content": [{ "type": "text", "text": format!("file error: {e}") }], "isError": true })
                }
            }
        }
        "tarang_analyze" => {
            let path = args["path"].as_str().unwrap_or("");
            match std::fs::File::open(path) {
                Ok(file) => match tarang_audio::probe_audio(file) {
                    Ok(info) => {
                        let analysis = tarang_ai::analyze_media(&info);
                        json!({
                            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&analysis).unwrap_or_default() }]
                        })
                    }
                    Err(e) => {
                        json!({ "content": [{ "type": "text", "text": format!("error: {e}") }], "isError": true })
                    }
                },
                Err(e) => {
                    json!({ "content": [{ "type": "text", "text": format!("file error: {e}") }], "isError": true })
                }
            }
        }
        "tarang_codecs" => {
            let audio: Vec<String> = tarang_audio::supported_codecs()
                .iter()
                .map(|c| c.to_string())
                .collect();
            let video: Vec<String> = tarang_video::supported_codecs()
                .iter()
                .map(|(c, b)| format!("{c} ({b})"))
                .collect();
            json!({
                "content": [{ "type": "text", "text": format!("Audio (pure Rust): {}\nVideo (C FFI): {}", audio.join(", "), video.join(", ")) }]
            })
        }
        "tarang_transcribe" => {
            let path = args["path"].as_str().unwrap_or("");
            let lang = args["language"].as_str().map(String::from);
            match std::fs::File::open(path) {
                Ok(file) => match tarang_audio::probe_audio(file) {
                    Ok(info) => match tarang_ai::prepare_transcription(&info, lang) {
                        Some(req) => json!({
                            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&req).unwrap_or_default() }]
                        }),
                        None => {
                            json!({ "content": [{ "type": "text", "text": "no audio stream found" }], "isError": true })
                        }
                    },
                    Err(e) => {
                        json!({ "content": [{ "type": "text", "text": format!("error: {e}") }], "isError": true })
                    }
                },
                Err(e) => {
                    json!({ "content": [{ "type": "text", "text": format!("file error: {e}") }], "isError": true })
                }
            }
        }
        "tarang_formats" => {
            let path = args["path"].as_str().unwrap_or("");
            match std::fs::read(path) {
                Ok(data) => {
                    let header = &data[..data.len().min(32)];
                    match tarang_core::ContainerFormat::from_magic(header) {
                        Some(fmt) => json!({
                            "content": [{ "type": "text", "text": format!("Detected: {fmt} (extensions: {})", fmt.extensions().join(", ")) }]
                        }),
                        None => {
                            json!({ "content": [{ "type": "text", "text": "unknown format" }], "isError": true })
                        }
                    }
                }
                Err(e) => {
                    json!({ "content": [{ "type": "text", "text": format!("file error: {e}") }], "isError": true })
                }
            }
        }
        // Async tools — need to be handled via a helper
        "tarang_fingerprint_index" | "tarang_search_similar" | "tarang_describe" => {
            json!({ "content": [{ "type": "text", "text": "async tool — use handle_async_tool_call" }], "isError": true })
        }
        _ => {
            json!({ "content": [{ "type": "text", "text": format!("unknown tool: {name}") }], "isError": true })
        }
    }
}

async fn handle_async_tool_call(name: &str, args: &serde_json::Value) -> serde_json::Value {
    use serde_json::json;

    match name {
        "tarang_fingerprint_index" => {
            let path = args["path"].as_str().unwrap_or("");
            let file = match std::fs::File::open(path) {
                Ok(f) => f,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("file error: {e}") }], "isError": true }),
            };
            let info = match tarang_audio::probe_audio(file) {
                Ok(i) => i,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("probe error: {e}") }], "isError": true }),
            };

            // Decode audio and compute fingerprint
            let buffer = match tarang_audio::FileDecoder::open_path(std::path::Path::new(path)).and_then(|mut d| d.decode_all()) {
                Ok(b) => b,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("decode error: {e}") }], "isError": true }),
            };

            let config = tarang_ai::FingerprintConfig::default();
            let fingerprint = match tarang_ai::compute_fingerprint(&buffer, &config) {
                Ok(fp) => fp,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("fingerprint error: {e}") }], "isError": true }),
            };

            let analysis = tarang_ai::analyze_media(&info);
            let daimon = match tarang_ai::DaimonClient::new(tarang_ai::DaimonConfig::default()) {
                Ok(c) => c,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("daimon client error: {e}") }], "isError": true }),
            };

            // Ensure collection exists, then index
            let _ = daimon.ensure_collection().await;

            let metadata = json!({
                "content_type": analysis.content_type.to_string(),
                "quality_score": analysis.quality_score,
                "tags": analysis.tags,
                "duration_secs": fingerprint.duration_secs,
            });

            match daimon.index_fingerprint(path, &fingerprint, &metadata).await {
                Ok(_) => {
                    // Also ingest into RAG
                    let _ = daimon.ingest_metadata(path, &info, &analysis).await;
                    json!({
                        "content": [{ "type": "text", "text": format!(
                            "Indexed: {path}\nFingerprint: {} hashes ({:.1}s)\nContent: {}\nAlso ingested into RAG pipeline",
                            fingerprint.hashes.len(), fingerprint.duration_secs, analysis.content_type
                        )}]
                    })
                }
                Err(e) => json!({ "content": [{ "type": "text", "text": format!("index error: {e}") }], "isError": true }),
            }
        }
        "tarang_search_similar" => {
            let path = args["path"].as_str().unwrap_or("");
            let top_k = args["top_k"].as_u64().unwrap_or(5) as usize;

            let buffer = match tarang_audio::FileDecoder::open_path(std::path::Path::new(path)).and_then(|mut d| d.decode_all()) {
                Ok(b) => b,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("decode error: {e}") }], "isError": true }),
            };

            let config = tarang_ai::FingerprintConfig::default();
            let fingerprint = match tarang_ai::compute_fingerprint(&buffer, &config) {
                Ok(fp) => fp,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("fingerprint error: {e}") }], "isError": true }),
            };

            let daimon = match tarang_ai::DaimonClient::new(tarang_ai::DaimonConfig::default()) {
                Ok(c) => c,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("daimon client error: {e}") }], "isError": true }),
            };

            match daimon.search_similar(&fingerprint, top_k).await {
                Ok(results) => json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&results).unwrap_or_default() }]
                }),
                Err(e) => json!({ "content": [{ "type": "text", "text": format!("search error: {e}") }], "isError": true }),
            }
        }
        "tarang_describe" => {
            let path = args["path"].as_str().unwrap_or("");
            let file = match std::fs::File::open(path) {
                Ok(f) => f,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("file error: {e}") }], "isError": true }),
            };
            let info = match tarang_audio::probe_audio(file) {
                Ok(i) => i,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("probe error: {e}") }], "isError": true }),
            };

            let analysis = tarang_ai::analyze_media(&info);
            let hoosh = match tarang_ai::HooshLlmClient::new(tarang_ai::HooshLlmConfig::default()) {
                Ok(c) => c,
                Err(e) => return json!({ "content": [{ "type": "text", "text": format!("hoosh client error: {e}") }], "isError": true }),
            };

            match hoosh.describe_content(&info, &analysis).await {
                Ok(desc) => json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&desc).unwrap_or_default() }]
                }),
                Err(e) => json!({ "content": [{ "type": "text", "text": format!("describe error: {e}") }], "isError": true }),
            }
        }
        _ => json!({ "content": [{ "type": "text", "text": format!("unknown async tool: {name}") }], "isError": true }),
    }
}
