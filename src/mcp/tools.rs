use serde_json::{Value, json};
use std::fs::File;
use tarang_core::MediaInfo;

/// Build an MCP error response.
pub fn error_response(msg: impl std::fmt::Display) -> Value {
    json!({ "content": [{ "type": "text", "text": msg.to_string() }], "isError": true })
}

/// Build an MCP success response.
pub fn success_response(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }] })
}

/// Extract a required `path` string from tool arguments.
pub fn require_path(args: &Value) -> Result<&str, Value> {
    match args.get("path").and_then(|p| p.as_str()) {
        Some(p) if !p.is_empty() => Ok(p),
        _ => Err(error_response("missing required parameter: path")),
    }
}

/// Open a file and probe its media info. Returns the open file handle and
/// parsed [`MediaInfo`], or an MCP error [`Value`] on failure.
pub fn open_and_probe(path: &str) -> Result<(File, MediaInfo), Value> {
    let file = File::open(path).map_err(|e| error_response(format!("file error: {e}")))?;
    let info =
        tarang_audio::probe_audio(file).map_err(|e| error_response(format!("probe error: {e}")))?;
    // Re-open so callers can still use the file if needed (probe consumed the first handle)
    let file = File::open(path).map_err(|e| error_response(format!("file error: {e}")))?;
    Ok((file, info))
}

pub fn handle_tool_call(name: &str, args: &Value) -> Value {
    match name {
        "tarang_probe" => {
            let path = match require_path(args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match open_and_probe(path) {
                Ok((_file, info)) => {
                    success_response(serde_json::to_string_pretty(&info).unwrap_or_default())
                }
                Err(e) => e,
            }
        }
        "tarang_analyze" => {
            let path = match require_path(args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match open_and_probe(path) {
                Ok((_file, info)) => {
                    let analysis = tarang_ai::analyze_media(&info);
                    success_response(serde_json::to_string_pretty(&analysis).unwrap_or_default())
                }
                Err(e) => e,
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
            success_response(format!(
                "Audio (pure Rust): {}\nVideo (C FFI): {}",
                audio.join(", "),
                video.join(", ")
            ))
        }
        "tarang_transcribe" => {
            let path = match require_path(args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let lang = args["language"].as_str().map(String::from);
            match open_and_probe(path) {
                Ok((_file, info)) => match tarang_ai::prepare_transcription(&info, lang) {
                    Some(req) => {
                        success_response(serde_json::to_string_pretty(&req).unwrap_or_default())
                    }
                    None => error_response("no audio stream found"),
                },
                Err(e) => e,
            }
        }
        "tarang_formats" => {
            let path = match require_path(args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            match std::fs::read(path) {
                Ok(data) => {
                    let header = &data[..data.len().min(32)];
                    match tarang_core::ContainerFormat::from_magic(header) {
                        Some(fmt) => success_response(format!(
                            "Detected: {fmt} (extensions: {})",
                            fmt.extensions().join(", ")
                        )),
                        None => error_response("unknown format"),
                    }
                }
                Err(e) => error_response(format!("file error: {e}")),
            }
        }
        // Async tools — need to be handled via handle_async_tool_call
        "tarang_fingerprint_index" | "tarang_search_similar" | "tarang_describe" => {
            error_response("async tool — use handle_async_tool_call")
        }
        _ => error_response(format!("unknown tool: {name}")),
    }
}

pub async fn handle_async_tool_call(name: &str, args: &Value) -> Value {
    match name {
        "tarang_fingerprint_index" => {
            let path = match require_path(args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let (_file, info) = match open_and_probe(path) {
                Ok(v) => v,
                Err(e) => return e,
            };

            // Decode audio and compute fingerprint
            let buffer = match tarang_audio::FileDecoder::open_path(std::path::Path::new(path))
                .and_then(|mut d| d.decode_all())
            {
                Ok(b) => b,
                Err(e) => return error_response(format!("decode error: {e}")),
            };

            let config = tarang_ai::FingerprintConfig::default();
            let fingerprint = match tarang_ai::compute_fingerprint(&buffer, &config) {
                Ok(fp) => fp,
                Err(e) => return error_response(format!("fingerprint error: {e}")),
            };

            let analysis = tarang_ai::analyze_media(&info);
            let daimon = match tarang_ai::DaimonClient::new(tarang_ai::DaimonConfig::default()) {
                Ok(c) => c,
                Err(e) => return error_response(format!("daimon client error: {e}")),
            };

            // Ensure collection exists, then index
            if let Err(e) = daimon.ensure_collection().await {
                tracing::warn!("Failed to ensure vector collection: {e}");
            }

            let metadata = json!({
                "content_type": analysis.content_type.to_string(),
                "quality_score": analysis.quality_score,
                "tags": analysis.tags,
                "duration_secs": fingerprint.duration_secs,
            });

            match daimon
                .index_fingerprint(path, &fingerprint, &metadata)
                .await
            {
                Ok(_) => {
                    // Also ingest into RAG
                    if let Err(e) = daimon.ingest_metadata(path, &info, &analysis).await {
                        tracing::warn!("RAG ingest failed for {path}: {e}");
                    }
                    success_response(format!(
                        "Indexed: {path}\nFingerprint: {} hashes ({:.1}s)\nContent: {}\nAlso ingested into RAG pipeline",
                        fingerprint.hashes.len(),
                        fingerprint.duration_secs,
                        analysis.content_type
                    ))
                }
                Err(e) => error_response(format!("index error: {e}")),
            }
        }
        "tarang_search_similar" => {
            let path = match require_path(args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let top_k = args["top_k"].as_u64().unwrap_or(5) as usize;

            let buffer = match tarang_audio::FileDecoder::open_path(std::path::Path::new(path))
                .and_then(|mut d| d.decode_all())
            {
                Ok(b) => b,
                Err(e) => return error_response(format!("decode error: {e}")),
            };

            let config = tarang_ai::FingerprintConfig::default();
            let fingerprint = match tarang_ai::compute_fingerprint(&buffer, &config) {
                Ok(fp) => fp,
                Err(e) => return error_response(format!("fingerprint error: {e}")),
            };

            let daimon = match tarang_ai::DaimonClient::new(tarang_ai::DaimonConfig::default()) {
                Ok(c) => c,
                Err(e) => return error_response(format!("daimon client error: {e}")),
            };

            match daimon.search_similar(&fingerprint, top_k).await {
                Ok(results) => {
                    success_response(serde_json::to_string_pretty(&results).unwrap_or_default())
                }
                Err(e) => error_response(format!("search error: {e}")),
            }
        }
        "tarang_describe" => {
            let path = match require_path(args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let (_file, info) = match open_and_probe(path) {
                Ok(v) => v,
                Err(e) => return e,
            };

            let analysis = tarang_ai::analyze_media(&info);
            let hoosh = match tarang_ai::HooshLlmClient::new(tarang_ai::HooshLlmConfig::default()) {
                Ok(c) => c,
                Err(e) => return error_response(format!("hoosh client error: {e}")),
            };

            match hoosh.describe_content(&info, &analysis).await {
                Ok(desc) => {
                    success_response(serde_json::to_string_pretty(&desc).unwrap_or_default())
                }
                Err(e) => error_response(format!("describe error: {e}")),
            }
        }
        _ => error_response(format!("unknown async tool: {name}")),
    }
}
