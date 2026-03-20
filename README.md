# Tarang

[![Crates.io](https://img.shields.io/crates/v/tarang.svg)](https://crates.io/crates/tarang)
[![Docs.rs](https://docs.rs/tarang/badge.svg)](https://docs.rs/tarang)
[![CI](https://github.com/MacCracken/tarang/actions/workflows/ci.yml/badge.svg)](https://github.com/MacCracken/tarang/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/MacCracken/tarang/graph/badge.svg)](https://codecov.io/gh/MacCracken/tarang)
[![License: GPL-3.0](https://img.shields.io/crates/l/tarang.svg)](LICENSE)

**AI-native Rust media framework for AGNOS**

Tarang (Sanskrit: *wave*) is a modular media engine that replaces ffmpeg with a Rust-owned pipeline. Pure Rust audio decoding via symphonia, pure Rust container demuxing/muxing, thin C FFI wrappers for video codecs, and AI-powered content analysis via the AGNOS agent ecosystem.

## Installation

Add tarang to your project:

```toml
[dependencies]
tarang = "0.19"
```

Or with specific codec backends:

```toml
[dependencies]
tarang = { version = "0.19", features = ["dav1d", "openh264"] }
```

## Modules

| Module | Description |
|--------|-------------|
| `tarang::core` | Codecs, formats, buffers, errors, YUV420p helpers |
| `tarang::demux` | Container demux/mux (MP4, MKV/WebM, OGG, WAV — all pure Rust) |
| `tarang::audio` | Decode, encode, resample, mix, PipeWire output |
| `tarang::video` | Video decode/encode (dav1d, openh264, libvpx, rav1e, VA-API) |
| `tarang::ai` | Content classification, fingerprinting, scene detection, thumbnails, transcription |

## Audio Codecs (Pure Rust)

| Codec | Decode | Encode |
|-------|--------|--------|
| MP3   | symphonia | - |
| FLAC  | symphonia | pure Rust |
| Vorbis| symphonia | - |
| Opus  | symphonia | libopus FFI (`opus-enc`) |
| AAC   | symphonia | fdk-aac FFI (`aac-enc`) |
| ALAC  | symphonia | - |
| PCM   | symphonia | pure Rust (16/24/32-bit) |

## Video Codecs (C FFI)

| Codec | Decode | Encode | Feature Flag |
|-------|--------|--------|-------------|
| AV1   | dav1d  | rav1e (pure Rust) | `dav1d` / `rav1e` |
| H.264 | openh264 | openh264 | `openh264` / `openh264-enc` |
| VP8   | libvpx | libvpx | `vpx` / `vpx-enc` |
| VP9   | libvpx | libvpx | `vpx` / `vpx-enc` |
| H.265 | - | VA-API scaffolding | `vaapi` |

## Container Formats (Pure Rust)

| Format | Demux | Mux |
|--------|-------|-----|
| MP4/M4A | full | full |
| MKV/WebM | full (EBML) | full |
| OGG (Vorbis/Opus/FLAC) | full (CRC-32 validated, bisection seek) | full (CRC-32) |
| WAV | full | full |
| FLAC | magic detection | - |
| MP3 | magic detection (ID3 + sync word) | - |
| AVI | magic detection | - |

## AI Features

| Feature | API | Description |
|---------|-----|-------------|
| Content classification | `ai::analyze_media` | Music/Speech/Podcast/Movie/Clip/Animation detection |
| Audio fingerprinting | `ai::compute_fingerprint` | Chromaprint-style FFT→chroma→hash, similarity matching |
| Scene detection | `ai::SceneDetector` | Chi-squared histogram distance, gradual transition detection |
| Thumbnail generation | `ai::ThumbnailGenerator` | Variance-scored keyframes, JPEG/PNG output |
| Transcription | `ai::HooshClient` | Audio preprocessing + chunked routing to hoosh (Whisper) |
| Vector search | `ai::DaimonClient` | Fingerprint indexing + similarity search via AGNOS daimon |
| RAG ingestion | `ai::DaimonClient` | Media metadata ingestion for natural-language queries |
| LLM description | `ai::HooshLlmClient` | Content description via hoosh LLM gateway |

## Usage

```bash
# Probe a media file
tarang probe song.flac

# AI content analysis
tarang analyze movie.mp4

# List supported codecs
tarang codecs

# Run as MCP server (JSON-RPC over stdio)
tarang mcp
```

## Building

```bash
# Default (audio only, no video FFI)
cargo build

# All codecs
cargo build --features full

# Specific codecs
cargo build --features "dav1d,openh264,vpx"

# With PipeWire audio output
cargo build --features pipewire

# With audio encoders
cargo build --features "opus-enc,aac-enc"
```

## Testing

```bash
make check                    # format + clippy + test + audit
cargo test                    # 500+ tests
cargo test --features full    # includes feature-gated codec tests
```

## Why Not FFmpeg?

ffmpeg is a 500K+ line C monolith. Tarang takes the opposite approach:

- **Smaller attack surface** — only the codecs you need, behind feature flags
- **Memory safety** — pipeline is Rust; only codec math is C FFI
- **Modularity** — each codec is a separate dependency
- **AGNOS-native** — fits the Rust-first, security-conscious OS
- **AI-first** — content analysis, fingerprinting, and transcription are built in

## Downstream Consumers

- **Jalwa** — AGNOS media player (primary consumer)
- **Tazama** — video editor (decode/encode pipeline)
- **Shruti** — DAW (audio I/O unified under tarang audio module)

## MSRV

Rust 1.89 (edition 2024).

## License

GPL-3.0
