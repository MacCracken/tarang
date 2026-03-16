# Tarang Architecture

## Crate Dependency Graph

```
tarang-core (types, codecs, buffers, errors)
  ↑
  ├── tarang-demux (container parsing — MP4, MKV, WebM, OGG, WAV)
  ├── tarang-audio (pure Rust decoding via symphonia)
  ├── tarang-video (C FFI wrappers: dav1d, openh264, libvpx)
  └── tarang-ai (classification, quality analysis, transcription)
        ↑
        └── main binary (CLI + MCP server)
```

## Design Principles

1. **Rust-owned pipeline** — The orchestration layer, memory management, and error handling are all in safe Rust. Only codec math is delegated to C.

2. **Pure Rust where possible** — Audio decoding is 100% Rust via symphonia. Container demuxing is pure Rust. No C code for anything that isn't performance-critical codec math.

3. **Thin FFI for video codecs** — Video decoders use small, focused C libraries with clean APIs:
   - **dav1d** for AV1 (VideoLAN, BSD-2-Clause)
   - **openh264** for H.264 (Cisco, BSD-2-Clause)
   - **libvpx** for VP8/VP9 (Google, BSD-3-Clause)

4. **Incremental replacement** — As Rust codec implementations mature, C FFI backends can be swapped out without changing the pipeline.

5. **AI-native** — Content analysis, transcription routing, and codec recommendations are first-class features, not plugins.

## Why Not FFmpeg?

FFmpeg is a monolithic 500K+ line C codebase that bundles container parsing, codec decoding, filtering, encoding, and muxing into one binary. Tarang takes the opposite approach:

- **Smaller attack surface** — Only link the specific codecs needed
- **Memory safety** — Pipeline code is Rust; only codec math is C
- **Modularity** — Each codec is a separate dependency, not a monolith
- **AGNOS alignment** — Fits the Rust-first, security-conscious OS philosophy
