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
                    }
                ]
            }),
            "tools/call" => {
                let tool_name = request["params"]["name"].as_str().unwrap_or("");
                let args = &request["params"]["arguments"];
                handle_tool_call(tool_name, args)
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
        _ => {
            json!({ "content": [{ "type": "text", "text": format!("unknown tool: {name}") }], "isError": true })
        }
    }
}
