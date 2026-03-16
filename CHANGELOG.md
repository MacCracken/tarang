# Changelog

## 2026.3.16b

Video codec bindings — V3, F1, F2 complete.

### New features
- VP8/VP9 encoding via libvpx FFI (`vpx-enc` feature flag)
- H.264 decoding via openh264 (`openh264` feature flag, Cisco BSD-2-Clause)
- H.264 encoding via openh264 (`openh264-enc` feature flag)
- `full` convenience feature flag enables all codec backends
- VPX ABI version auto-detection via build.rs probe (with env-var override for cross-compilation)

### Safety & correctness (3 rounds of code review)
- RAII guard (`VpxImageGuard`) for vpx image lifetime — panic-safe
- Bounds checks on all frame data before unsafe plane copies
- Signed (`isize`) stride arithmetic in all vpx FFI — handles negative strides
- Pixel format validation: dav1d rejects non-I420, vpx_dec rejects non-I420, openh264_enc requires YUV420p
- Ceiling division for chroma dimensions in all decoders (correct for odd sizes)
- Frame dimension and data size validation in all encoders
- Zero-dimension and zero-framerate rejection
- `vpx_codec_control_` return value checked with proper cleanup on failure
- `data.len() > u32::MAX` guard in vpx decoder
- `DecoderConfig::for_codec()` now validates that the required feature is enabled
- `supported_codecs()` only lists compiled-in backends
- `unsafe impl Send` for VpxEncoder/VpxDecoder (single-owner contexts)
- Removed redundant `initialized` field from vpx types
- openh264 decoder: `flush()` method drains buffered frames with correct timestamps
- openh264 encoder: zero-copy `YUVSlices` instead of `YUVBuffer::from_vec`
- openh264 encoder: uses `encode_at()` with frame timestamps
- vpx encoder: uses frame timestamps for PTS
- dav1d decoder: clamps negative timestamps to 0
- dav1d decoder: documents timestamp unit contract
- build.rs: `rerun-if-changed` directives, `$CC` respect, both-or-neither env-var check
- Fixed dav1d stride type mismatch (`u32` → `usize` cast for dav1d 0.11)
- Fixed vpx crate name (`vpx_sys` not `libvpx_sys`)

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
