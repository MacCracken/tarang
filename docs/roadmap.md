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
- [x] VA-API hardware acceleration detection (behind `vaapi` feature flag, probes DRM render nodes)
- ~~VDPAU~~ — dropped (Mesa removed VDPAU from all open-source drivers)

### F2: Remaining Video Encoding
- [x] openh264 bindings (H.264 encoding, behind `openh264-enc` feature flag)
- [x] VA-API hardware-accelerated encode scaffolding (H.264/HEVC, surface lifecycle, entrypoint selection)

### F3: AI Features
- [x] Transcription routing to hoosh (Whisper models — HooshClient, WAV encoding, audio preprocessing)
- [x] Audio fingerprinting (Chromaprint-style — FFT, chroma features, hash comparison)
- [x] Scene detection in video (histogram diff, chi-squared distance, gradual transitions)
- [x] Thumbnail generation at keyframes (YUV→RGB, JPEG/PNG encoding, variance scoring)

## Post-v1 — Ecosystem Integration (Complete)
- [x] AGNOS media player backend (Jalwa — built on tarang)
- [x] Tazama video editor backend (tarang decode/encode pipeline)
- [x] Shruti DAW backend (audio I/O unified under tarang-audio)
- [x] AGNOS marketplace recipe (`recipes/marketplace/tarang.toml`)
- [x] MCP tools registered in daimon (8 tools: probe, analyze, codecs, transcribe, formats, fingerprint_index, search_similar, describe)
- [x] agnoshi intents (8 intents: probe, analyze, codecs, transcribe, formats, fingerprint, similar, describe)
- [x] Daimon integration module — vector store fingerprint indexing, RAG metadata ingestion, multimodal agent registration, LLM content description via hoosh
- [x] Hoosh endpoint fix (port 8088, correct path)

## Waiting on Upstream
- [ ] **VA-API encode pipeline completion** — surface upload, parameter buffers, bitstream readback. Blocked on `cros-codecs` releasing a version compatible with `cros-libva` 0.0.13 (current cros-codecs 0.0.6 depends on cros-libva 0.0.12). *(added 2026-03-16)*

## Engineering Backlog *(added 2026-03-16)*

### Remaining items
- [ ] Deduplicate `bytes_to_f32`/`f32_to_bytes` helpers (6+ copies) — extract shared module or use `bytemuck`
- [ ] PipeWire ring buffer: replace `Mutex` with lock-free `AtomicUsize` for read/write positions
- [ ] PipeWire: replace blocking `flush()` sleep loop with condvar/channel notification
- [ ] PipeWire: replace hardcoded 50ms init sleep with proper ready signal
- [ ] Probe: detect actual format instead of hardcoding `ContainerFormat::Mp4`
- [ ] Return `Cow`/reference instead of cloning `AudioBuffer` in `resample()`/`mix_channels()` no-op paths
- [ ] OGG: implement CRC-32 page validation
- [ ] OGG: bisection seek (currently O(n) linear scan)
- [ ] Thumbnail: avoid cloning full `VideoFrame` — use `Arc` or store metadata only
- [ ] Complete VA-API encode pipeline (surface upload → encode → readback) — blocked on upstream cros-codecs
- [ ] Complete `VideoDecoder` implementation (currently stubs)

### Completed
- [x] Add `Copy` derive to `OutputConfig` and `EncoderConfig`
- [x] FLAC encoder: log warning on silent zero-padding
- [x] OGG: randomize serial number
- [x] OGG: bounds-check `try_into().unwrap()` → proper error propagation
- [x] MP4: validate allocation sizes (64 MB guard)
- [x] MP4: replace `.unwrap()` on `playback.as_mut()` with error propagation
- [x] MKV: saturating timecode arithmetic to prevent i64 overflow
- [x] OGG muxer: validate codec at construction
- [x] Eliminate unnecessary `info.clone()` in all demuxers
- [x] Add SAFETY comments to FFI unsafe blocks in vpx_dec.rs, vpx_enc.rs
- [x] Validate rav1e even dimensions
- [x] VPX encoder: validate `bitrate_bps >= 1000`
- [x] Fingerprint: add max hash count limit (64K)
- [x] Fingerprint: remove unused `_num_bands` parameter
- [x] Scene detection: bounds-check RGB24 pixel data length
- [x] Daimon: improve error context (operation + path in messages)
- [x] Daimon: validate config endpoint URLs at construction time
- [x] Document content-type thresholds (named constants with comments)
- [x] WebM vs MKV detection — EBML DocType parsing

## Downstream Consumers (All Integrated)
- **AGNOS Media Player (Jalwa)** — primary GUI player built on tarang
- **Tazama** — video editor, using tarang for decode/encode pipeline
- **Shruti** — DAW, audio I/O unified under tarang-audio

> **Note**: As tarang gains new capabilities (e.g. new codecs, hardware acceleration, streaming), review downstream consumers for integration updates.
