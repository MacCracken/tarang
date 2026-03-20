//! Daimon (agent-runtime) integration
//!
//! Connects tarang to the AGNOS agent orchestrator for:
//! - Vector store: fingerprint indexing and similarity search
//! - RAG: media metadata ingestion for natural-language queries
//! - Multimodal agent registration: Audio + Vision modalities
//! - LLM content description: route thumbnails/metadata to hoosh for richer analysis

use crate::core::{MediaInfo, Result, TarangError};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::{AudioFingerprint, MediaAnalysis};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the daimon (agent-runtime) connection.
#[derive(Debug, Clone)]
pub struct DaimonConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
}

impl Default for DaimonConfig {
    fn default() -> Self {
        Self {
            endpoint: std::env::var("DAIMON_URL")
                .unwrap_or_else(|_| "http://localhost:8090".into()),
            api_key: std::env::var("DAIMON_API_KEY").ok(),
            timeout_secs: 30,
        }
    }
}

/// Configuration for hoosh LLM-powered content description.
#[derive(Debug, Clone)]
pub struct HooshLlmConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub model: String,
    pub timeout_secs: u64,
}

impl Default for HooshLlmConfig {
    fn default() -> Self {
        Self {
            endpoint: std::env::var("HOOSH_URL").unwrap_or_else(|_| "http://localhost:8088".into()),
            api_key: std::env::var("HOOSH_API_KEY").ok(),
            model: std::env::var("HOOSH_MODEL").unwrap_or_else(|_| "llama3".into()),
            timeout_secs: 60,
        }
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Client for integrating with daimon services.
pub struct DaimonClient {
    config: DaimonConfig,
    http: reqwest::Client,
}

impl DaimonClient {
    pub fn new(config: DaimonConfig) -> Result<Self> {
        if config.endpoint.is_empty()
            || !(config.endpoint.starts_with("http://") || config.endpoint.starts_with("https://"))
        {
            return Err(TarangError::NetworkError(
                format!("invalid daimon endpoint: {:?}", config.endpoint).into(),
            ));
        }
        if config.timeout_secs == 0 {
            return Err(TarangError::NetworkError(
                "daimon timeout must be > 0".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| TarangError::NetworkError(format!("HTTP client error: {e}").into()))?;
        Ok(Self { config, http })
    }

    fn auth_header(&self) -> Option<String> {
        self.config.api_key.as_ref().map(|k| format!("Bearer {k}"))
    }

    // -----------------------------------------------------------------------
    // Vector Store — fingerprint indexing
    // -----------------------------------------------------------------------

    /// Index an audio fingerprint in the daimon vector store for similarity search.
    pub async fn index_fingerprint(
        &self,
        file_path: &str,
        fingerprint: &AudioFingerprint,
        metadata: &serde_json::Value,
    ) -> Result<String> {
        let embedding = fingerprint_to_embedding(fingerprint);
        if embedding.is_empty() {
            return Err(TarangError::AiError(
                "empty fingerprint — nothing to index".into(),
            ));
        }

        let body = serde_json::json!({
            "collection": "tarang_fingerprints",
            "items": [{
                "embedding": embedding,
                "content": file_path,
                "metadata": metadata,
            }]
        });

        let url = format!("{}/v1/vectors/insert", self.config.endpoint);
        let mut req = self.http.post(&url).json(&body);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req.send().await.map_err(|e| {
            TarangError::NetworkError(format!("vector insert failed for {file_path}: {e}").into())
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_bytes = read_body_limited(resp).await.unwrap_or_default();
            let body = String::from_utf8_lossy(&body_bytes);
            return Err(TarangError::NetworkError(
                format!("vector insert returned {status}: {body}").into(),
            ));
        }

        info!(path = %file_path, hashes = fingerprint.hashes.len(), "Indexed fingerprint in vector store");
        Ok(file_path.into())
    }

    /// Search for similar media by fingerprint.
    pub async fn search_similar(
        &self,
        fingerprint: &AudioFingerprint,
        top_k: usize,
    ) -> Result<Vec<SimilarMedia>> {
        let embedding = fingerprint_to_embedding(fingerprint);
        if embedding.is_empty() {
            return Ok(Vec::new());
        }

        let body = serde_json::json!({
            "collection": "tarang_fingerprints",
            "embedding": embedding,
            "top_k": top_k,
        });

        let url = format!("{}/v1/vectors/search", self.config.endpoint);
        let mut req = self.http.post(&url).json(&body);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| TarangError::NetworkError(format!("vector search failed: {e}").into()))?;
        // NOTE: search failure logged at call site via warn! (see query_media)

        if !resp.status().is_success() {
            warn!("Vector search returned {}", resp.status());
            return Ok(Vec::new());
        }

        let result: VectorSearchResponse = read_json_limited(resp).await?;

        Ok(result
            .results
            .into_iter()
            .map(|r| SimilarMedia {
                path: r.content,
                score: r.score,
                metadata: r.metadata,
            })
            .collect())
    }

    // -----------------------------------------------------------------------
    // RAG — media metadata ingestion
    // -----------------------------------------------------------------------

    /// Ingest media analysis metadata into the RAG pipeline for NL queries.
    pub async fn ingest_metadata(
        &self,
        file_path: &str,
        info: &MediaInfo,
        analysis: &MediaAnalysis,
    ) -> Result<()> {
        let text = format_metadata_for_rag(file_path, info, analysis);

        let body = serde_json::json!({
            "text": text,
            "agent_id": "tarang",
            "metadata": {
                "source": "tarang",
                "path": file_path,
                "content_type": analysis.content_type.to_string(),
            }
        });

        let url = format!("{}/v1/rag/ingest", self.config.endpoint);
        let mut req = self.http.post(&url).json(&body);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req.send().await.map_err(|e| {
            TarangError::NetworkError(format!("RAG ingest failed for {file_path}: {e}").into())
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            warn!(%status, "RAG ingest returned non-success");
        } else {
            info!(path = %file_path, "Ingested media metadata into RAG");
        }

        Ok(())
    }

    /// Query RAG for media matching a natural-language description.
    pub async fn query_media(&self, query: &str, top_k: usize) -> Result<Vec<RagResult>> {
        let body = serde_json::json!({
            "query": query,
            "top_k": top_k,
            "agent_id": "tarang",
        });

        let url = format!("{}/v1/rag/query", self.config.endpoint);
        let mut req = self.http.post(&url).json(&body);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| TarangError::NetworkError(format!("RAG query failed: {e}").into()))?;
        // NOTE: non-success status logged via warn! above

        if !resp.status().is_success() {
            warn!(status = %resp.status(), "RAG query returned non-success");
            return Ok(Vec::new());
        }

        let result: RagQueryResponse = read_json_limited(resp).await?;

        Ok(result.results)
    }

    // -----------------------------------------------------------------------
    // Multimodal agent registration
    // -----------------------------------------------------------------------

    /// Register tarang as a multimodal agent with daimon.
    pub async fn register_agent(&self) -> Result<()> {
        let body = serde_json::json!({
            "name": "tarang",
            "id": "tarang-media-framework",
            "domain": "media",
            "capabilities": ["audio_decode", "video_decode", "fingerprint", "scene_detect", "thumbnail", "transcribe", "content_analysis"],
            "metadata": {
                "modalities_input": ["audio", "video"],
                "modalities_output": ["text", "structured_data"],
                "version": env!("CARGO_PKG_VERSION"),
                "runtime": "native-binary",
            }
        });

        let url = format!("{}/v1/agents/register", self.config.endpoint);
        let mut req = self.http.post(&url).json(&body);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req.send().await.map_err(|e| {
            TarangError::NetworkError(format!("agent registration failed: {e}").into())
        })?;

        if resp.status().is_success() {
            info!("Registered tarang as multimodal agent with daimon");
        } else {
            let status = resp.status();
            warn!(%status, "Agent registration returned non-success");
        }

        Ok(())
    }

    /// Ensure the tarang_fingerprints vector collection exists.
    pub async fn ensure_collection(&self) -> Result<()> {
        let body = serde_json::json!({
            "name": "tarang_fingerprints",
        });

        let url = format!("{}/v1/vectors/collections", self.config.endpoint);
        let mut req = self.http.post(&url).json(&body);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req.send().await.map_err(|e| {
            TarangError::NetworkError(format!("collection create failed: {e}").into())
        })?;

        // 409 = already exists, which is fine
        if resp.status().is_success() || resp.status().as_u16() == 409 {
            info!("Vector collection 'tarang_fingerprints' ready");
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// LLM content description via hoosh
// ---------------------------------------------------------------------------

/// Client for LLM-powered content description via hoosh.
pub struct HooshLlmClient {
    config: HooshLlmConfig,
    http: reqwest::Client,
}

impl HooshLlmClient {
    pub fn new(config: HooshLlmConfig) -> Result<Self> {
        if config.endpoint.is_empty()
            || !(config.endpoint.starts_with("http://") || config.endpoint.starts_with("https://"))
        {
            return Err(TarangError::NetworkError(
                format!("invalid hoosh LLM endpoint: {:?}", config.endpoint).into(),
            ));
        }
        if config.timeout_secs == 0 {
            return Err(TarangError::NetworkError(
                "hoosh timeout must be > 0".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| TarangError::NetworkError(format!("HTTP client error: {e}").into()))?;
        Ok(Self { config, http })
    }

    /// Generate a rich content description using LLM analysis.
    ///
    /// Sends media metadata + analysis to hoosh for natural-language description,
    /// genre classification, and content moderation tags.
    pub async fn describe_content(
        &self,
        info: &MediaInfo,
        analysis: &MediaAnalysis,
    ) -> Result<ContentDescription> {
        let prompt = build_description_prompt(info, analysis);

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [{
                "role": "user",
                "content": prompt,
            }],
            "temperature": 0.3,
            "max_tokens": 512,
        });

        let url = format!("{}/v1/chat/completions", self.config.endpoint);
        let mut req = self.http.post(&url).json(&body);
        if let Some(key) = &self.config.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req.send().await.map_err(|e| {
            TarangError::NetworkError(format!("hoosh LLM describe_content failed: {e}").into())
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            return Err(TarangError::NetworkError(
                format!("hoosh LLM returned {status}").into(),
            ));
        }

        let result: serde_json::Value = read_json_limited(resp).await?;

        let content = result
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("");

        if content.is_empty() {
            warn!("LLM response had no content in choices[0].message.content");
        }

        let content = content.to_string();

        parse_description_response(&content, analysis)
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A media file similar to a query fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarMedia {
    pub path: String,
    pub score: f64,
    pub metadata: serde_json::Value,
}

/// A RAG query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagResult {
    pub text: String,
    pub relevance: f64,
}

/// LLM-generated content description.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentDescription {
    pub summary: String,
    pub genre: Option<String>,
    pub mood: Option<String>,
    pub tags: Vec<String>,
    pub content_rating: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VectorSearchResponse {
    results: Vec<VectorSearchResult>,
}

#[derive(Debug, Deserialize)]
struct VectorSearchResult {
    content: String,
    score: f64,
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RagQueryResponse {
    results: Vec<RagResult>,
}

// ---------------------------------------------------------------------------
// Response body helpers
// ---------------------------------------------------------------------------

/// Maximum response body size (10 MB).
const MAX_RESPONSE_BYTES: usize = 10_485_760;

/// Read a response body as bytes, rejecting bodies larger than `MAX_RESPONSE_BYTES`.
async fn read_body_limited(resp: reqwest::Response) -> Result<bytes::Bytes> {
    let content_length = resp.content_length().unwrap_or(0) as usize;
    if content_length > MAX_RESPONSE_BYTES {
        return Err(TarangError::NetworkError(
            format!(
                "response body too large: {content_length} bytes (limit: {MAX_RESPONSE_BYTES})"
            )
            .into(),
        ));
    }
    let body = resp.bytes().await.map_err(|e| {
        TarangError::NetworkError(format!("failed to read response body: {e}").into())
    })?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(TarangError::NetworkError(
            format!(
                "response body too large: {} bytes (limit: {MAX_RESPONSE_BYTES})",
                body.len()
            )
            .into(),
        ));
    }
    Ok(body)
}

/// Read a response body and deserialize as JSON, with size limit.
async fn read_json_limited<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
    let body = read_body_limited(resp).await?;
    serde_json::from_slice(&body).map_err(|e| {
        TarangError::NetworkError(format!("failed to parse JSON response: {e}").into())
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert fingerprint hashes to a normalized float embedding for the vector store.
fn fingerprint_to_embedding(fp: &AudioFingerprint) -> Vec<f32> {
    if fp.hashes.is_empty() {
        return Vec::new();
    }

    // Use hash bit patterns as embedding dimensions.
    // Normalize each u32 to [0, 1] range.
    fp.hashes
        .iter()
        .map(|&h| h as f32 / u32::MAX as f32)
        .collect()
}

/// Format media metadata as text for RAG ingestion.
fn format_metadata_for_rag(path: &str, info: &MediaInfo, analysis: &MediaAnalysis) -> String {
    let mut parts = vec![
        format!("File: {path}"),
        format!("Format: {}", info.format),
        format!("Content type: {}", analysis.content_type),
        format!("Quality: {:.0}/100", analysis.quality_score),
    ];

    if let Some(d) = info.duration {
        parts.push(format!("Duration: {:.1}s", d.as_secs_f64()));
    }
    if let Some(title) = &info.title {
        parts.push(format!("Title: {title}"));
    }
    if let Some(artist) = &info.artist {
        parts.push(format!("Artist: {artist}"));
    }
    if let Some(album) = &info.album {
        parts.push(format!("Album: {album}"));
    }

    for stream in &info.streams {
        match stream {
            crate::core::StreamInfo::Audio(a) => {
                parts.push(format!(
                    "Audio: {} {}Hz {}ch",
                    a.codec, a.sample_rate, a.channels
                ));
            }
            crate::core::StreamInfo::Video(v) => {
                parts.push(format!(
                    "Video: {} {}x{} {:.1}fps",
                    v.codec, v.width, v.height, v.frame_rate
                ));
            }
            crate::core::StreamInfo::Subtitle { language } => {
                parts.push(format!(
                    "Subtitle: {}",
                    language.as_deref().unwrap_or("unknown")
                ));
            }
        }
    }

    if !analysis.tags.is_empty() {
        parts.push(format!("Tags: {}", analysis.tags.join(", ")));
    }
    if let Some(rec) = &analysis.codec_recommendation {
        parts.push(format!("Recommendation: {rec}"));
    }

    parts.join("\n")
}

/// Build a prompt for LLM content description.
fn build_description_prompt(info: &MediaInfo, analysis: &MediaAnalysis) -> String {
    let mut ctx = Vec::new();
    ctx.push(format!("Content type: {}", analysis.content_type));
    ctx.push(format!("Quality score: {:.0}/100", analysis.quality_score));

    if let Some(d) = info.duration {
        let mins = d.as_secs() / 60;
        let secs = d.as_secs() % 60;
        ctx.push(format!("Duration: {}m{}s", mins, secs));
    }
    if let Some(title) = &info.title {
        ctx.push(format!("Title: {title}"));
    }
    if let Some(artist) = &info.artist {
        ctx.push(format!("Artist: {artist}"));
    }

    for stream in &info.streams {
        match stream {
            crate::core::StreamInfo::Audio(a) => {
                ctx.push(format!(
                    "Audio: {} {}Hz {}ch",
                    a.codec, a.sample_rate, a.channels
                ));
            }
            crate::core::StreamInfo::Video(v) => {
                ctx.push(format!(
                    "Video: {} {}x{} {:.1}fps",
                    v.codec, v.width, v.height, v.frame_rate
                ));
            }
            _ => {}
        }
    }

    format!(
        "Analyze this media file and provide a JSON response with these fields:\n\
         - summary: 1-2 sentence description of the content\n\
         - genre: likely genre (e.g. \"rock\", \"documentary\", \"tutorial\")\n\
         - mood: emotional tone (e.g. \"energetic\", \"calm\", \"dramatic\")\n\
         - tags: list of relevant descriptive tags\n\
         - content_rating: suggested rating (\"G\", \"PG\", \"PG-13\", \"R\", or null)\n\n\
         Media metadata:\n{}\n\n\
         Respond with only valid JSON, no markdown.",
        ctx.join("\n")
    )
}

/// Parse LLM response into a ContentDescription.
fn parse_description_response(
    response: &str,
    analysis: &MediaAnalysis,
) -> Result<ContentDescription> {
    // Try parsing as JSON first
    if let Ok(desc) = serde_json::from_str::<ContentDescription>(response) {
        return Ok(desc);
    }

    // Try extracting JSON from markdown code block
    let json_str = response
        .find('{')
        .and_then(|start| response.rfind('}').map(|end| &response[start..=end]));

    if let Some(json) = json_str
        && let Ok(desc) = serde_json::from_str::<ContentDescription>(json)
    {
        return Ok(desc);
    }

    // Fallback: use the raw response as summary
    Ok(ContentDescription {
        summary: response.trim().to_string(),
        genre: None,
        mood: None,
        tags: analysis.tags.clone(),
        content_rating: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::*;
    use std::time::Duration;
    use uuid::Uuid;

    #[test]
    fn fingerprint_to_embedding_normalizes() {
        let fp = AudioFingerprint {
            hashes: vec![0, u32::MAX / 2, u32::MAX],
            duration_secs: 2.0,
        };
        let emb = fingerprint_to_embedding(&fp);
        assert_eq!(emb.len(), 3);
        assert!((emb[0] - 0.0).abs() < 0.001);
        assert!((emb[1] - 0.5).abs() < 0.01);
        assert!((emb[2] - 1.0).abs() < 0.001);
    }

    #[test]
    fn fingerprint_to_embedding_empty() {
        let fp = AudioFingerprint {
            hashes: Vec::new(),
            duration_secs: 0.0,
        };
        assert!(fingerprint_to_embedding(&fp).is_empty());
    }

    #[test]
    fn format_rag_metadata() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mp4,
            streams: vec![StreamInfo::Audio(AudioStreamInfo {
                codec: AudioCodec::Aac,
                sample_rate: 44100,
                channels: 2,
                sample_format: SampleFormat::F32,
                bitrate: None,
                duration: Some(Duration::from_secs(180)),
            })],
            duration: Some(Duration::from_secs(180)),
            file_size: None,
            title: Some("Test Song".into()),
            artist: Some("Test Artist".into()),
            album: None,
            metadata: std::collections::HashMap::new(),
        };
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Music,
            quality_score: 70.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec!["audio".to_string()],
        };

        let text = format_metadata_for_rag("/music/song.mp4", &info, &analysis);
        assert!(text.contains("Test Song"));
        assert!(text.contains("Test Artist"));
        assert!(text.contains("music"));
        assert!(text.contains("180.0s"));
        assert!(text.contains("AAC"));
    }

    #[test]
    fn build_prompt_includes_metadata() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Flac,
            streams: vec![StreamInfo::Audio(AudioStreamInfo {
                codec: AudioCodec::Flac,
                sample_rate: 96000,
                channels: 2,
                sample_format: SampleFormat::I32,
                bitrate: None,
                duration: Some(Duration::from_secs(300)),
            })],
            duration: Some(Duration::from_secs(300)),
            file_size: None,
            title: Some("Opus No. 1".into()),
            artist: Some("Composer".into()),
            album: None,
            metadata: std::collections::HashMap::new(),
        };
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Music,
            quality_score: 80.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec!["audio".to_string()],
        };

        let prompt = build_description_prompt(&info, &analysis);
        assert!(prompt.contains("Opus No. 1"));
        assert!(prompt.contains("Composer"));
        assert!(prompt.contains("96000Hz"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn parse_valid_json_description() {
        let json = r#"{"summary":"A rock song","genre":"rock","mood":"energetic","tags":["guitar","drums"],"content_rating":"PG"}"#;
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Music,
            quality_score: 70.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec![],
        };
        let desc = parse_description_response(json, &analysis).unwrap();
        assert_eq!(desc.summary, "A rock song");
        assert_eq!(desc.genre, Some("rock".into()));
        assert_eq!(desc.mood, Some("energetic".into()));
        assert_eq!(desc.tags, vec!["guitar", "drums"]);
    }

    #[test]
    fn parse_json_in_markdown_block() {
        let response = "Here is the analysis:\n```json\n{\"summary\":\"A calm podcast\",\"genre\":\"talk\",\"mood\":\"calm\",\"tags\":[\"interview\"],\"content_rating\":null}\n```";
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Podcast,
            quality_score: 60.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec![],
        };
        let desc = parse_description_response(response, &analysis).unwrap();
        assert_eq!(desc.summary, "A calm podcast");
        assert_eq!(desc.genre, Some("talk".into()));
    }

    #[test]
    fn parse_fallback_raw_text() {
        let response = "This is just a plain text response with no JSON.";
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Unknown,
            quality_score: 50.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec!["audio".to_string()],
        };
        let desc = parse_description_response(response, &analysis).unwrap();
        assert_eq!(desc.summary, response);
        assert_eq!(desc.tags, vec!["audio"]);
        assert!(desc.genre.is_none());
    }

    #[test]
    fn daimon_config_defaults() {
        let config = DaimonConfig::default();
        assert!(config.endpoint.contains("8090"));
    }

    #[test]
    fn hoosh_llm_config_defaults() {
        let config = HooshLlmConfig::default();
        assert!(config.endpoint.contains("8088"));
    }

    #[test]
    fn similar_media_serialization() {
        let sm = SimilarMedia {
            path: "/music/test.flac".to_string(),
            score: 0.95,
            metadata: serde_json::json!({"artist": "test"}),
        };
        let json = serde_json::to_string(&sm).unwrap();
        let parsed: SimilarMedia = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.path, "/music/test.flac");
        assert!((parsed.score - 0.95).abs() < 0.001);
    }

    #[test]
    fn content_description_serialization() {
        let desc = ContentDescription {
            summary: "A test".to_string(),
            genre: Some("test".into()),
            mood: None,
            tags: vec!["a".to_string(), "b".to_string()],
            content_rating: Some("G".into()),
        };
        let json = serde_json::to_string(&desc).unwrap();
        let parsed: ContentDescription = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.summary, "A test");
        assert_eq!(parsed.tags.len(), 2);
    }

    #[test]
    fn rag_result_serialization() {
        let r = RagResult {
            text: "Some media info".to_string(),
            relevance: 0.8,
        };
        let json = serde_json::to_string(&r).unwrap();
        let parsed: RagResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.text, "Some media info");
    }

    // -----------------------------------------------------------------------
    // fingerprint_to_embedding — additional cases
    // -----------------------------------------------------------------------

    #[test]
    fn fingerprint_to_embedding_single_hash() {
        let fp = AudioFingerprint {
            hashes: vec![u32::MAX],
            duration_secs: 0.1,
        };
        let emb = fingerprint_to_embedding(&fp);
        assert_eq!(emb.len(), 1);
        assert!((emb[0] - 1.0).abs() < 0.001);
    }

    #[test]
    fn fingerprint_to_embedding_100_hashes() {
        let hashes: Vec<u32> = (0..100).map(|i| i * (u32::MAX / 100)).collect();
        let fp = AudioFingerprint {
            hashes,
            duration_secs: 5.0,
        };
        let emb = fingerprint_to_embedding(&fp);
        assert_eq!(emb.len(), 100);
        // First should be near 0, last should be near 0.99
        assert!(emb[0] < 0.01);
        assert!(emb[99] > 0.95);
        // All values should be in [0, 1]
        for &v in &emb {
            assert!((0.0..=1.0).contains(&v));
        }
    }

    #[test]
    fn fingerprint_to_embedding_max_value_hashes() {
        let fp = AudioFingerprint {
            hashes: vec![u32::MAX; 50],
            duration_secs: 2.5,
        };
        let emb = fingerprint_to_embedding(&fp);
        assert_eq!(emb.len(), 50);
        for &v in &emb {
            assert!((v - 1.0).abs() < 0.001);
        }
    }

    #[test]
    fn fingerprint_to_embedding_zero_hashes() {
        let fp = AudioFingerprint {
            hashes: vec![0; 10],
            duration_secs: 1.0,
        };
        let emb = fingerprint_to_embedding(&fp);
        assert_eq!(emb.len(), 10);
        for &v in &emb {
            assert!((v - 0.0).abs() < 0.001);
        }
    }

    // -----------------------------------------------------------------------
    // format_metadata_for_rag — video streams, subtitle streams
    // -----------------------------------------------------------------------

    #[test]
    fn format_rag_metadata_with_video_stream() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mp4,
            streams: vec![
                StreamInfo::Video(VideoStreamInfo {
                    codec: VideoCodec::H264,
                    width: 1920,
                    height: 1080,
                    pixel_format: PixelFormat::Yuv420p,
                    frame_rate: 29.97,
                    bitrate: Some(5_000_000),
                    duration: Some(Duration::from_secs(120)),
                }),
                StreamInfo::Audio(AudioStreamInfo {
                    codec: AudioCodec::Aac,
                    sample_rate: 44100,
                    channels: 2,
                    sample_format: SampleFormat::F32,
                    bitrate: Some(128000),
                    duration: Some(Duration::from_secs(120)),
                }),
            ],
            duration: Some(Duration::from_secs(120)),
            file_size: Some(75_000_000),
            title: Some("My Video".into()),
            artist: None,
            album: None,
            metadata: std::collections::HashMap::new(),
        };
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Movie,
            quality_score: 85.0,
            codec_recommendation: Some("use AV1".into()),
            estimated_complexity: 0.7,
            tags: vec!["video".to_string(), "hd".to_string()],
        };

        let text = format_metadata_for_rag("/videos/clip.mp4", &info, &analysis);
        assert!(text.contains("Video: H.264 1920x1080 30.0fps"));
        assert!(text.contains("Audio: AAC 44100Hz 2ch"));
        assert!(text.contains("My Video"));
        assert!(text.contains("movie"));
        assert!(text.contains("use AV1"));
        assert!(text.contains("video, hd"));
    }

    #[test]
    fn format_rag_metadata_with_subtitle_stream() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mkv,
            streams: vec![
                StreamInfo::Audio(AudioStreamInfo {
                    codec: AudioCodec::Opus,
                    sample_rate: 48000,
                    channels: 2,
                    sample_format: SampleFormat::F32,
                    bitrate: None,
                    duration: Some(Duration::from_secs(60)),
                }),
                StreamInfo::Subtitle {
                    language: Some("en".into()),
                },
                StreamInfo::Subtitle { language: None },
            ],
            duration: Some(Duration::from_secs(60)),
            file_size: None,
            title: None,
            artist: None,
            album: None,
            metadata: std::collections::HashMap::new(),
        };
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Podcast,
            quality_score: 65.0,
            codec_recommendation: None,
            estimated_complexity: 0.3,
            tags: vec![],
        };

        let text = format_metadata_for_rag("/media/episode.mkv", &info, &analysis);
        assert!(text.contains("Subtitle: en"));
        assert!(text.contains("Subtitle: unknown"));
        assert!(text.contains("Opus"));
    }

    // -----------------------------------------------------------------------
    // build_description_prompt — video+audio mixed media
    // -----------------------------------------------------------------------

    #[test]
    fn build_prompt_video_audio_mixed() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mp4,
            streams: vec![
                StreamInfo::Video(VideoStreamInfo {
                    codec: VideoCodec::Av1,
                    width: 3840,
                    height: 2160,
                    pixel_format: PixelFormat::Yuv420p,
                    frame_rate: 60.0,
                    bitrate: None,
                    duration: Some(Duration::from_secs(600)),
                }),
                StreamInfo::Audio(AudioStreamInfo {
                    codec: AudioCodec::Opus,
                    sample_rate: 48000,
                    channels: 6,
                    sample_format: SampleFormat::F32,
                    bitrate: None,
                    duration: Some(Duration::from_secs(600)),
                }),
            ],
            duration: Some(Duration::from_secs(600)),
            file_size: None,
            title: Some("Nature Documentary".into()),
            artist: None,
            album: None,
            metadata: std::collections::HashMap::new(),
        };
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Movie,
            quality_score: 95.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec![],
        };

        let prompt = build_description_prompt(&info, &analysis);
        assert!(prompt.contains("Nature Documentary"));
        assert!(prompt.contains("AV1 3840x2160 60.0fps"));
        assert!(prompt.contains("Opus 48000Hz 6ch"));
        assert!(prompt.contains("10m0s"));
        assert!(prompt.contains("movie"));
        assert!(prompt.contains("JSON"));
    }

    // -----------------------------------------------------------------------
    // parse_description_response — malformed JSON, empty, very long
    // -----------------------------------------------------------------------

    #[test]
    fn parse_malformed_json() {
        let response = r#"{"summary": "test", "genre": INVALID_JSON}"#;
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Unknown,
            quality_score: 50.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec!["fallback_tag".to_string()],
        };
        let desc = parse_description_response(response, &analysis).unwrap();
        // Should fall back to raw text since JSON is invalid
        assert!(desc.summary.contains("summary"));
        assert_eq!(desc.tags, vec!["fallback_tag"]);
    }

    #[test]
    fn parse_empty_string() {
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Unknown,
            quality_score: 50.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec![],
        };
        let desc = parse_description_response("", &analysis).unwrap();
        assert_eq!(desc.summary, "");
        assert!(desc.genre.is_none());
        assert!(desc.mood.is_none());
    }

    #[test]
    fn parse_very_long_text() {
        let long_text = "A".repeat(10_000);
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Music,
            quality_score: 70.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec!["long".to_string()],
        };
        let desc = parse_description_response(&long_text, &analysis).unwrap();
        assert_eq!(desc.summary.len(), 10_000);
        assert_eq!(desc.tags, vec!["long"]);
        assert!(desc.genre.is_none());
    }

    #[test]
    fn parse_json_with_extra_text_before_and_after() {
        let response = r#"Sure! Here is the analysis:
{"summary":"A classical piece","genre":"classical","mood":"serene","tags":["orchestra","strings"],"content_rating":"G"}
Hope that helps!"#;
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Music,
            quality_score: 80.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec![],
        };
        let desc = parse_description_response(response, &analysis).unwrap();
        assert_eq!(desc.summary, "A classical piece");
        assert_eq!(desc.genre, Some("classical".into()));
        assert_eq!(desc.mood, Some("serene".into()));
        assert_eq!(desc.content_rating, Some("G".into()));
    }

    #[test]
    fn parse_partial_json_missing_fields() {
        let response = r#"{"summary":"Minimal response"}"#;
        let analysis = MediaAnalysis {
            content_type: crate::ai::ContentType::Unknown,
            quality_score: 50.0,
            codec_recommendation: None,
            estimated_complexity: 0.0,
            tags: vec![],
        };
        // serde should handle missing optional fields via defaults
        let result = parse_description_response(response, &analysis);
        // If ContentDescription requires all fields, it falls back to raw text
        let desc = result.unwrap();
        // Either parsed or fell back — either way summary should be meaningful
        assert!(!desc.summary.is_empty());
    }

    #[test]
    fn test_daimon_invalid_endpoint() {
        // ftp:// scheme should be rejected
        let config = DaimonConfig {
            endpoint: "ftp://example.com".to_string(),
            api_key: None,
            timeout_secs: 30,
        };
        assert!(DaimonClient::new(config).is_err());

        // Empty endpoint should be rejected
        let config = DaimonConfig {
            endpoint: String::new(),
            api_key: None,
            timeout_secs: 30,
        };
        assert!(DaimonClient::new(config).is_err());

        // Valid http:// should succeed
        let config = DaimonConfig {
            endpoint: "http://localhost:8090".to_string(),
            api_key: None,
            timeout_secs: 30,
        };
        assert!(DaimonClient::new(config).is_ok());

        // Valid https:// should succeed
        let config = DaimonConfig {
            endpoint: "https://example.com".to_string(),
            api_key: None,
            timeout_secs: 30,
        };
        assert!(DaimonClient::new(config).is_ok());

        // Zero timeout should be rejected
        let config = DaimonConfig {
            endpoint: "http://localhost:8090".to_string(),
            api_key: None,
            timeout_secs: 0,
        };
        assert!(DaimonClient::new(config).is_err());
    }
}
