# Tarang Architecture

## Crate Dependency Graph

```
tarang-core (types, codecs, buffers, errors, YUV420p helpers, dimension validation)
  ^
  +-- tarang-demux (container demux + mux: MP4, MKV/WebM, OGG, WAV — all pure Rust)
  +-- tarang-audio (decode, encode, resample, mix, PipeWire output, probe)
  |     +-- sample.rs (shared bytes_to_f32, f32_to_bytes, PCM scaling constants, test helpers)
  |     +-- decode.rs (symphonia-based FileDecoder)
  |     +-- encode.rs + encode_flac/opus/aac (PCM, FLAC pure Rust, Opus + AAC FFI)
  |     +-- resample.rs (linear + windowed sinc)
  |     +-- mix.rs (stereo/mono/5.1/generic channel mixing)
  |     +-- output/ (AudioOutput trait, NullOutput, PipeWire lock-free SPSC)
  |     +-- probe.rs (symphonia metadata extraction, format auto-detection)
  +-- tarang-video (video decode + encode via C FFI)
  |     +-- lib.rs (unified VideoDecoder dispatching to backends)
  |     +-- dav1d_dec.rs (AV1 decode)
  |     +-- openh264_dec.rs / openh264_enc.rs (H.264)
  |     +-- vpx_dec.rs / vpx_enc.rs (VP8/VP9)
  |     +-- rav1e_enc.rs (AV1 encode, pure Rust)
  |     +-- vaapi_enc.rs / vaapi_probe.rs (VA-API HW acceleration)
  +-- tarang-ai (AI-powered media analysis)
        +-- lib.rs (analyze_media, content classification)
        +-- fingerprint.rs (audio fingerprinting: FFT, chroma, hashing)
        +-- scene.rs (scene boundary detection: histogram chi-squared)
        +-- thumbnail.rs (keyframe selection, YUV->RGB, JPEG/PNG encoding)
        +-- transcribe.rs (audio preprocessing, WAV encoding, hoosh routing)
        +-- daimon.rs (vector store, RAG ingestion, agent registration, LLM description)
        ^
        +-- main binary (CLI: probe, analyze, codecs, mcp)
```

## Design Principles

1. **Rust-owned pipeline** — Orchestration, memory management, and error handling are all in safe Rust. Only codec math is delegated to C.

2. **Pure Rust where possible** — Audio decoding is 100% Rust via symphonia. Container demuxing/muxing is pure Rust. No C code for anything that isn't performance-critical codec math.

3. **Thin FFI for video codecs** — Video decoders use small, focused C libraries with clean APIs:
   - **dav1d** for AV1 (VideoLAN, BSD-2-Clause)
   - **openh264** for H.264 (Cisco, BSD-2-Clause)
   - **libvpx** for VP8/VP9 (Google, BSD-3-Clause)
   - **rav1e** for AV1 encoding (pure Rust, BSD-2-Clause)

4. **Unified VideoDecoder** — A single `VideoDecoder` type dispatches to the appropriate FFI backend based on codec. Handles buffered frames, flush/drain, and dimension auto-detection.

5. **Feature flags** — Each video codec backend is behind a Cargo feature flag (`dav1d`, `openh264`, `vpx`, `rav1e`, `vpx-enc`, `openh264-enc`, `vaapi`, `pipewire`, `opus-enc`, `aac-enc`). Only the codecs you need are compiled.

6. **AI-native** — Content analysis, fingerprinting, scene detection, transcription routing, and vector search are first-class features integrated with the AGNOS agent ecosystem (daimon, hoosh).

7. **Incremental replacement** — As Rust codec implementations mature, C FFI backends can be swapped out without changing the pipeline.

## Why Not FFmpeg?

FFmpeg is a monolithic 500K+ line C codebase that bundles container parsing, codec decoding, filtering, encoding, and muxing into one binary. Tarang takes the opposite approach:

- **Smaller attack surface** — Only link the specific codecs needed
- **Memory safety** — Pipeline code is Rust; only codec math is C
- **Modularity** — Each codec is a separate dependency, not a monolith
- **AGNOS alignment** — Fits the Rust-first, security-conscious OS philosophy
- **Audited** — 2 security audit rounds, 310 tests, zero clippy warnings

## Key Internal Patterns

- **`sample.rs`** — Single source of truth for `bytes_to_f32` / `f32_to_bytes` conversions and PCM scaling constants (`I16_SCALE`, `I24_SCALE`, `I32_SCALE`). All audio modules import from here.
- **`yuv420p_frame_size()`** — Canonical frame size calculation using ceiling division for chroma planes. Used by all video encoders.
- **`validate_video_dimensions()`** — Shared non-zero + even dimension check for all video encoders.
- **Lock-free PipeWire** — SPSC ring buffer with `AtomicUsize` positions, condvar-based readiness signaling, deadline-based flush.
- **OGG CRC-32** — Const lookup table shared between demuxer (validation) and muxer (generation).
