# Tarang Roadmap

> **Principle**: FOSS codecs first, proprietary codecs next, wide coverage always in scope.

Completed items are in [CHANGELOG.md](../../CHANGELOG.md).

---

## Pre-v1 (0.20–0.x)

### AI features

- [ ] **Offline transcription** — bundle a small Whisper model (tiny/base) for local inference without hoosh dependency

### Release prep

- [ ] **Publish 0.20.3 to crates.io** — verify `cargo publish --dry-run` passes; tag and push

---

## v1.0.0 criteria

All of the following must be true before cutting 1.0:

- [ ] Public API reviewed and marked stable (no `#[non_exhaustive]` additions expected)
- [ ] All `Demuxer`/`Muxer`/`AudioEncoder` traits finalized
- [ ] Core types (`AudioBuffer`, `VideoFrame`, `MediaInfo`, `Packet`) frozen
- [ ] 95%+ line coverage
- [ ] All demuxer fuzz targets passing with 0 crashes after 1M iterations
- [ ] At least one downstream consumer (Jalwa, Tazama, or Shruti) running on stable tarang
- [ ] docs.rs documentation complete with examples for every public module
- [ ] No `unsafe` blocks without `// SAFETY:` comments
- [ ] `cargo-vet` fully audited (zero exemptions for direct dependencies)
- [ ] SemVer compliance enforced via `cargo-semver-checks` in CI

---

## Post-v1

Longer-term items that don't block any release.

### New codec backends

- [ ] **AV1 decode via rav1e** — if rav1e adds decode support, replace dav1d for pure-Rust AV1
- [ ] **Opus decode via pure Rust** — replace symphonia's Opus with a dedicated decoder for lower latency
- [ ] **FLAC decode via tarang** — replace symphonia's FLAC with our own encoder running in reverse (roundtrip validation)
- [ ] **VP8/VP9 pure Rust** — when a viable pure-Rust VP8/VP9 decoder exists, add as alternative to libvpx

### Platform support

- [ ] **macOS CoreAudio output** — alternative to PipeWire for macOS
- [ ] **Windows WASAPI output** — alternative to PipeWire for Windows
- [ ] **Android MediaCodec** — hardware decode/encode via Android NDK
- [ ] **iOS AVFoundation** — hardware decode/encode via Apple frameworks
- [ ] **WASM target** — browser-based media processing with Web Audio API

### Advanced features

- [ ] **Muxer streaming** — write to `AsyncWrite` for network streaming (HLS, DASH, WebRTC)
- [ ] **Parallel decode** — multi-threaded packet decode for multi-core utilization
- [ ] **GPU-accelerated fingerprinting** — compute FFT on GPU via Vulkan compute or CUDA
- [ ] **Real-time pipeline** — lock-free audio graph with deadline scheduling for live processing
- [ ] **Plugin system** — dynamic loading of codec/effect plugins at runtime
- [ ] **C FFI bindings** — `tarang.h` for C/C++ consumers
- [ ] **Python bindings** — PyO3 package for Python-based media analysis

### Performance

- [ ] **SIMD audio processing** — explicit SIMD for resample, mix, fingerprint inner loops (portable_simd or manual intrinsics)
- [ ] **Memory pool** — reusable frame/packet buffers to eliminate per-frame allocation
- [ ] **Zero-copy demux** — `mmap` + `Bytes::from_static` for reading packets without copying from kernel
- [ ] **Lazy metadata parsing** — parse only requested atoms/elements in MP4/MKV instead of full traversal

---

## Non-goals

- **Full ffmpeg replacement** — tarang covers decode, encode, demux, mux, and analysis. It does not aim to replace ffmpeg's filter graph, network protocols, or device capture.
- **Proprietary codec licensing** — no bundling of patent-encumbered codecs (Dolby, DTS). Only FOSS and permissive-licensed backends.
- **GUI** — tarang is a library and CLI. GUI media players (Jalwa) are separate projects.
- **Streaming server** — tarang produces segments/packets. Serving them over HTTP/WebRTC is the consumer's responsibility.
