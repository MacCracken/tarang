# Tarang MCP Tools

Tarang exposes 8 tools via the Model Context Protocol (JSON-RPC over stdio).

Run with `tarang mcp`.

## Tools

### tarang_probe
Probe a media file and return format, codec, duration, and stream info.

**Input**: `{ "path": "/path/to/file.mp4" }`
**Output**: Full `MediaInfo` JSON (format, streams, duration, metadata)

### tarang_analyze
AI-powered media content analysis — classify type (music/speech/podcast/movie/clip/animation), compute quality score, suggest codec improvements.

**Input**: `{ "path": "/path/to/file.mp4" }`
**Output**: `MediaAnalysis` JSON (content_type, quality_score, codec_recommendation, estimated_complexity, tags)

### tarang_codecs
List all supported audio and video codecs with their backends (pure Rust vs C FFI).

**Input**: `{}`
**Output**: Text listing audio codecs (pure Rust) and video codecs (C FFI)

### tarang_transcribe
Prepare a transcription request for audio content. Returns metadata for routing to hoosh (Whisper).

**Input**: `{ "path": "/path/to/file.mp3", "language": "en" }`
**Output**: `TranscriptionRequest` JSON (audio_codec, sample_rate, channels, duration_secs, language_hint)

### tarang_formats
Detect media container format from file header magic bytes.

**Input**: `{ "path": "/path/to/file" }`
**Output**: Detected format name and common file extensions

### tarang_fingerprint_index
Compute an audio fingerprint and index it in the AGNOS vector store (daimon) for similarity search. Also ingests metadata into the RAG pipeline.

**Input**: `{ "path": "/path/to/audio.flac" }`
**Output**: Confirmation with fingerprint hash count, duration, and content classification

### tarang_search_similar
Find media files similar to a given file using audio fingerprint matching against the AGNOS vector store.

**Input**: `{ "path": "/path/to/reference.flac", "top_k": 5 }`
**Output**: JSON array of similar media with paths, scores, and metadata

### tarang_describe
Generate a rich AI content description using LLM analysis via hoosh. Sends media metadata and analysis to the LLM for natural-language description, genre classification, and content moderation tags.

**Input**: `{ "path": "/path/to/file.mp4" }`
**Output**: `ContentDescription` JSON (summary, genre, mood, tags, content_rating)

## Protocol

Tarang implements the MCP protocol version `2024-11-05`:
- Transport: JSON-RPC 2.0 over stdin/stdout (one JSON object per line)
- Methods: `initialize`, `tools/list`, `tools/call`
- All file-based tools validate the `path` parameter and return structured errors for missing or invalid paths
