# Tarang MCP Tools

## Tools

### tarang_probe
Probe a media file and return format, codec, duration, and stream info.

**Input**: `{ "path": "/path/to/file.mp4" }`

### tarang_analyze
AI-powered media content analysis — classify type (music/speech/movie/clip), quality score, codec recommendations.

**Input**: `{ "path": "/path/to/file.mp4" }`

### tarang_codecs
List all supported audio and video codecs with their backends (pure Rust vs C FFI).

**Input**: `{}`

### tarang_transcribe
Prepare a transcription request for audio content. Returns metadata for routing to hoosh.

**Input**: `{ "path": "/path/to/file.mp3", "language": "en" }`

### tarang_formats
Detect media container format from file header magic bytes.

**Input**: `{ "path": "/path/to/file" }`
