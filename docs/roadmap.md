# Tarang Roadmap

> **Principle**: FOSS codecs first, proprietary codecs next, wide coverage always in scope.
> Audio pipeline is the critical path to MVP — video layers on after.
> Encoding follows decoding for each media type.

## Phase 1 — Foundation (Complete)
- [x] Core types: codecs, formats, buffers, pipeline primitives
- [x] WAV demuxer (pure Rust)
- [x] Magic bytes format detection
- [x] Audio probe via symphonia
- [x] Video decoder framework (backend stubs)
- [x] AI content classification
- [x] CLI + MCP server (5 tools)
- [x] CI/CD pipelines

## MVP (v0.1) — Audio-First Playback Engine

### M1: Container Demuxers
- [x] MP4/M4A demuxer (pure Rust — parse moov/mdat atoms)
- [x] OGG demuxer (pure Rust — page/packet extraction)

### M2: Full Audio Decode
- [x] Symphonia decode pipeline (full decode, not just probe)
- [x] FOSS first: FLAC, Vorbis, Opus, WAV/PCM
- [x] Then: MP3 (patents expired), AAC, ALAC

### M3: Audio Processing
- [x] Resampling (pure Rust — linear + windowed sinc)
- [x] Channel mixing (stereo↔mono, 5.1 downmix, generic N→1/N→2)

### M4: Audio Output
- [x] AudioOutput trait + NullOutput (test sink)
- [x] PipeWire output backend (behind `pipewire` feature flag, requires libpipewire-0.3)

### M5: Audio Encoding
- [x] Muxer trait + WAV muxer (pure Rust — inverse of demuxer, roundtrip-verified)
- [x] OGG muxer (pure Rust — page assembly, Opus + Vorbis headers, roundtrip-verified)
- [x] MP4/M4A muxer (pure Rust — moov/mdat assembly, roundtrip-verified with Mp4Demuxer)
- [x] PCM encoder (F32 → 16/24/32-bit interleaved PCM)
- [x] FLAC encoder (pure Rust — verbatim subframes, BitWriter, STREAMINFO generation)
- [x] Opus encoder (libopus FFI, behind `opus-enc` feature flag)
- [x] AAC encoder (fdk-aac FFI, behind `aac-enc` feature flag)

## v0.2 — Container Coverage + Video Bootstrap

### V1: MKV/WebM Demuxer
- [x] Matroska/WebM demuxer (pure Rust — EBML parser, audio + video tracks, SimpleBlock packets)

### V2: FOSS Video Codecs (Decode)
- [x] dav1d bindings (AV1 decoding, behind `dav1d` feature flag)
- [x] libvpx bindings (VP8/VP9 decoding, behind `vpx` feature flag)
- [x] Safe wrapper types with lifetime management

### V3: Video Encoding
- [x] rav1e bindings (AV1 encoding — pure Rust, behind `rav1e` feature flag)
- [x] libvpx-enc bindings (VP8/VP9 encoding, behind `vpx-enc` feature flag)
- [x] MKV/WebM muxer (pure Rust — EBML writer, roundtrip-verified with MkvDemuxer)

## v1.0 — Full Media Engine

### F1: Remaining Video Codecs (Decode)
- [x] openh264 bindings (H.264 decoding, behind `openh264` feature flag)
- [ ] Hardware acceleration detection (VA-API, VDPAU)

### F2: Remaining Video Encoding
- [x] openh264 bindings (H.264 encoding, behind `openh264-enc` feature flag)
- [ ] Hardware-accelerated encode (VA-API)

### F3: AI Features
- [ ] Transcription routing to hoosh (Whisper models)
- [ ] Audio fingerprinting
- [ ] Scene detection in video
- [ ] Thumbnail generation at keyframes

## Post-v1 — Ecosystem Integration
- [ ] AGNOS media player backend (primary consumer)
- [ ] Tazama video editor backend (replace GStreamer/ffmpeg with tarang)
- [ ] Shruti DAW backend (unify symphonia usage under tarang-audio)
- [ ] AGNOS marketplace recipe
- [ ] MCP tools registered in daimon
- [ ] agnoshi intents ("play music", "probe file", "transcribe audio")

## Downstream Consumers
- **AGNOS Media Player** (new, Priority 1) — primary GUI player built on tarang
- **Tazama** — video editor, migrate from GStreamer to tarang for decode/encode pipeline
- **Shruti** — DAW, unify audio I/O under tarang-audio (already uses symphonia)
