# Tarang

**AI-native Rust media framework for AGNOS**

Tarang (Sanskrit: *wave*) is a modular media engine that replaces ffmpeg with a Rust-owned pipeline. Pure Rust audio decoding via symphonia, pure Rust container demuxing/muxing, thin C FFI wrappers for video codecs, and AI-powered content analysis via the AGNOS agent ecosystem.

## Crates

```
tarang-core     — codecs, formats, buffers, errors, YUV420p helpers
tarang-demux    — container demux/mux (MP4, MKV/WebM, OGG, WAV, FLAC — all pure Rust)
tarang-audio    — decode, encode, resample, mix, PipeWire output
tarang-video    — video decode/encode (dav1d, openh264, libvpx, rav1e, VA-API)
tarang-ai       — content classification, fingerprinting, scene detection, thumbnails, transcription
```

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

| Feature | Module | Description |
|---------|--------|-------------|
| Content classification | `tarang-ai::analyze_media` | Music/Speech/Podcast/Movie/Clip/Animation detection |
| Audio fingerprinting | `tarang-ai::fingerprint` | Chromaprint-style FFT→chroma→hash, similarity matching |
| Scene detection | `tarang-ai::scene` | Chi-squared histogram distance, gradual transition detection |
| Thumbnail generation | `tarang-ai::thumbnail` | Variance-scored keyframes, JPEG/PNG output |
| Transcription | `tarang-ai::transcribe` | Audio preprocessing + routing to hoosh (Whisper) |
| Vector search | `tarang-ai::daimon` | Fingerprint indexing + similarity search via AGNOS daimon |
| RAG ingestion | `tarang-ai::daimon` | Media metadata ingestion for natural-language queries |
| LLM description | `tarang-ai::daimon` | Content description via hoosh LLM gateway |

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

## MCP Tools

| Tool | Description |
|------|-------------|
| `tarang_probe` | Probe media file for format, codec, duration, stream info |
| `tarang_analyze` | AI content classification and quality scoring |
| `tarang_codecs` | List supported audio/video codecs with backends |
| `tarang_transcribe` | Prepare transcription request (routes to hoosh) |
| `tarang_formats` | Detect container format from magic bytes |
| `tarang_fingerprint_index` | Compute audio fingerprint and index in AGNOS vector store |
| `tarang_search_similar` | Find similar media via fingerprint matching |
| `tarang_describe` | Generate AI content description via hoosh LLM |

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
cargo test --all                          # 310 tests
cargo test --all --all-features           # includes feature-gated codec tests
cargo clippy --all-targets --all-features # zero warnings
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
- **Shruti** — DAW (audio I/O unified under tarang-audio)

## License

GPL-3.0
