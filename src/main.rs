//! tarang — AI-native Rust media framework for AGNOS
//!
//! Tarang (Sanskrit: wave) provides media decoding with a Rust-owned pipeline,
//! pure Rust audio decoding via symphonia, and thin FFI wrappers for video codecs.

mod mcp;

use anyhow::Result;
use clap::{Parser, Subcommand};

// Re-export MCP tool functions so tests can use `super::*`
#[cfg(test)]
use mcp::{
    error_response, handle_async_tool_call, handle_tool_call, open_and_probe, require_path,
    success_response,
};

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
        Commands::Mcp => mcp::cmd_mcp().await?,
    }

    Ok(())
}

fn cmd_probe(path: &str) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let info = tarang::audio::probe_audio(file)?;

    println!("Format:   {}", info.format);
    if let Some(d) = info.duration {
        println!("Duration: {:.1}s", d.as_secs_f64());
    }
    println!("Streams:  {}", info.streams.len());

    for (i, stream) in info.streams.iter().enumerate() {
        match stream {
            tarang::core::StreamInfo::Audio(a) => {
                println!(
                    "  [{}] Audio: {} {}Hz {}ch",
                    i, a.codec, a.sample_rate, a.channels
                );
            }
            tarang::core::StreamInfo::Video(v) => {
                println!(
                    "  [{}] Video: {} {}x{} {:.1}fps",
                    i, v.codec, v.width, v.height, v.frame_rate
                );
            }
            tarang::core::StreamInfo::Subtitle { language } => {
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
    let info = tarang::audio::probe_audio(file)?;
    let analysis = tarang::ai::analyze_media(&info);

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
    for codec in tarang::audio::supported_codecs() {
        println!("  {codec}");
    }
    println!();
    println!("Video codecs (C FFI backends):");
    for (codec, backend) in tarang::video::supported_codecs() {
        println!("  {codec} — {backend}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    // ---- Helper: create a temp WAV file, return its path ----
    fn make_test_wav_file() -> tempfile::NamedTempFile {
        let bits: u16 = 16;
        let sample_rate: u32 = 44100;
        let channels: u16 = 2;
        let num_samples: u32 = 4410;
        let data_size = num_samples * channels as u32 * (bits as u32 / 8);
        let file_size = 36 + data_size;
        let byte_rate = sample_rate * channels as u32 * (bits as u32 / 8);
        let block_align = channels * (bits / 8);

        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for i in 0..num_samples {
            let t = i as f64 / sample_rate as f64;
            let s = (t * 440.0 * 2.0 * std::f64::consts::PI).sin();
            let s16 = (s * 32000.0) as i16;
            for _ in 0..channels {
                buf.extend_from_slice(&s16.to_le_bytes());
            }
        }

        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&buf).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    // ---- require_path ----

    #[test]
    fn require_path_valid() {
        let args = json!({"path": "/some/file.wav"});
        assert_eq!(require_path(&args).unwrap(), "/some/file.wav");
    }

    #[test]
    fn require_path_missing() {
        let args = json!({});
        let err = require_path(&args).unwrap_err();
        assert!(err["isError"].as_bool().unwrap());
    }

    #[test]
    fn require_path_empty() {
        let args = json!({"path": ""});
        assert!(require_path(&args).is_err());
    }

    #[test]
    fn require_path_null() {
        let args = json!({"path": null});
        assert!(require_path(&args).is_err());
    }

    #[test]
    fn require_path_number() {
        let args = json!({"path": 42});
        assert!(require_path(&args).is_err());
    }

    // ---- handle_tool_call: tarang_codecs ----

    #[test]
    fn tool_codecs() {
        let result = handle_tool_call("tarang_codecs", &json!({}));
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Audio"));
        assert!(text.contains("Video"));
    }

    // ---- handle_tool_call: unknown tool ----

    #[test]
    fn tool_unknown() {
        let result = handle_tool_call("nonexistent_tool", &json!({}));
        assert!(result["isError"].as_bool().unwrap());
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("unknown tool"));
    }

    // ---- handle_tool_call: missing path ----

    #[test]
    fn tool_probe_missing_path() {
        let result = handle_tool_call("tarang_probe", &json!({}));
        assert!(result["isError"].as_bool().unwrap());
    }

    #[test]
    fn tool_analyze_missing_path() {
        let result = handle_tool_call("tarang_analyze", &json!({}));
        assert!(result["isError"].as_bool().unwrap());
    }

    #[test]
    fn tool_transcribe_missing_path() {
        let result = handle_tool_call("tarang_transcribe", &json!({}));
        assert!(result["isError"].as_bool().unwrap());
    }

    #[test]
    fn tool_formats_missing_path() {
        let result = handle_tool_call("tarang_formats", &json!({}));
        assert!(result["isError"].as_bool().unwrap());
    }

    // ---- handle_tool_call: nonexistent file ----

    #[test]
    fn tool_probe_bad_file() {
        let result = handle_tool_call("tarang_probe", &json!({"path": "/nonexistent/file.wav"}));
        assert!(result["isError"].as_bool().unwrap());
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("file error"));
    }

    // ---- handle_tool_call: real WAV file ----

    #[test]
    fn tool_probe_wav() {
        let tmp = make_test_wav_file();
        let path = tmp.path().to_str().unwrap();
        let result = handle_tool_call("tarang_probe", &json!({"path": path}));
        assert!(result.get("isError").is_none());
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Wav"));
        assert!(text.contains("Pcm"));
    }

    #[test]
    fn tool_analyze_wav() {
        let tmp = make_test_wav_file();
        let path = tmp.path().to_str().unwrap();
        let result = handle_tool_call("tarang_analyze", &json!({"path": path}));
        assert!(result.get("isError").is_none());
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("content_type"));
        assert!(text.contains("quality_score"));
    }

    #[test]
    fn tool_transcribe_wav() {
        let tmp = make_test_wav_file();
        let path = tmp.path().to_str().unwrap();
        let result = handle_tool_call("tarang_transcribe", &json!({"path": path}));
        assert!(result.get("isError").is_none());
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("audio_codec"));
        assert!(text.contains("sample_rate"));
    }

    #[test]
    fn tool_transcribe_with_language() {
        let tmp = make_test_wav_file();
        let path = tmp.path().to_str().unwrap();
        let result = handle_tool_call(
            "tarang_transcribe",
            &json!({"path": path, "language": "en"}),
        );
        assert!(result.get("isError").is_none());
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("en"));
    }

    #[test]
    fn tool_formats_wav() {
        let tmp = make_test_wav_file();
        let path = tmp.path().to_str().unwrap();
        let result = handle_tool_call("tarang_formats", &json!({"path": path}));
        assert!(result.get("isError").is_none());
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("WAV"));
    }

    #[test]
    fn tool_formats_unknown() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"not a media file").unwrap();
        tmp.flush().unwrap();
        let path = tmp.path().to_str().unwrap();
        let result = handle_tool_call("tarang_formats", &json!({"path": path}));
        assert!(result["isError"].as_bool().unwrap());
    }

    // ---- async tool calls: missing path ----

    #[tokio::test]
    async fn async_tool_fingerprint_missing_path() {
        let result = handle_async_tool_call("tarang_fingerprint_index", &json!({})).await;
        assert!(result["isError"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn async_tool_search_missing_path() {
        let result = handle_async_tool_call("tarang_search_similar", &json!({})).await;
        assert!(result["isError"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn async_tool_describe_missing_path() {
        let result = handle_async_tool_call("tarang_describe", &json!({})).await;
        assert!(result["isError"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn async_tool_unknown() {
        let result = handle_async_tool_call("nonexistent", &json!({})).await;
        assert!(result["isError"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn async_tool_fingerprint_bad_file() {
        let result = handle_async_tool_call(
            "tarang_fingerprint_index",
            &json!({"path": "/nonexistent.wav"}),
        )
        .await;
        assert!(result["isError"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn async_tool_describe_bad_file() {
        let result =
            handle_async_tool_call("tarang_describe", &json!({"path": "/nonexistent.wav"})).await;
        assert!(result["isError"].as_bool().unwrap());
    }

    // ---- error_response / success_response ----

    #[test]
    fn error_response_structure() {
        let resp = error_response("something went wrong");
        assert!(resp["isError"].as_bool().unwrap());
        let content = &resp["content"];
        assert!(content.is_array());
        assert_eq!(content[0]["type"].as_str().unwrap(), "text");
        assert_eq!(content[0]["text"].as_str().unwrap(), "something went wrong");
    }

    #[test]
    fn success_response_structure() {
        let resp = success_response("all good");
        // success responses must NOT have isError
        assert!(resp.get("isError").is_none());
        let content = &resp["content"];
        assert!(content.is_array());
        assert_eq!(content[0]["type"].as_str().unwrap(), "text");
        assert_eq!(content[0]["text"].as_str().unwrap(), "all good");
    }

    // ---- open_and_probe ----

    #[test]
    fn open_and_probe_nonexistent() {
        let result = open_and_probe("/absolutely/nonexistent/file.wav");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err["isError"].as_bool().unwrap());
        let text = err["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("file error"));
    }

    #[test]
    fn open_and_probe_valid_wav() {
        let tmp = make_test_wav_file();
        let path = tmp.path().to_str().unwrap();
        let result = open_and_probe(path);
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.format.to_string(), "WAV");
        assert!(!info.streams.is_empty());
    }
}
