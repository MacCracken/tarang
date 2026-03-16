# Changelog

## 2026.3.16

Initial scaffolding.

- Core types: audio/video codecs, container formats, media buffers, pipeline primitives
- Container demuxing: WAV demuxer (pure Rust), format detection via magic bytes
- Audio decoding: symphonia integration (MP3, FLAC, WAV, OGG Vorbis, AAC, ALAC)
- Video decoding: decoder framework with dav1d/openh264/libvpx backend stubs
- AI features: content classification, quality scoring, codec recommendations, transcription prep
- CLI: probe, analyze, codecs, mcp subcommands
- MCP server: 5 tools (tarang_probe, tarang_analyze, tarang_codecs, tarang_transcribe, tarang_formats)
- CI/CD: GitHub Actions (check, test, clippy, fmt, multi-arch release)
