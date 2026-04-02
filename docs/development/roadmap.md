# Tarang Roadmap

> **Principle**: FOSS codecs first, proprietary codecs next, wide coverage always in scope.

Completed items are in [CHANGELOG.md](../../CHANGELOG.md).

---

## 0.21.3 — Pre-v1.0 hardening

- [ ] **Publish 0.21.3 to crates.io** — `cargo publish --dry-run`, tag, push

---

## 0.22.3

- [x] **Review ai-hwaccel feature set** — bumped to v1.0.0; added cached/lazy registry, detection warnings, report_from_registry, GPU backend kind for mabda pipeline

---

## v1.0.0 criteria

All of the following must be true before cutting 1.0:

- [ ] Public API reviewed and marked stable (no `#[non_exhaustive]` additions expected)
- [ ] All `Demuxer`/`Muxer`/`AudioEncoder` traits finalized
- [ ] Core types (`AudioBuffer`, `VideoFrame`, `MediaInfo`, `Packet`) frozen
- [ ] 89%+ line coverage (library code, excluding mcp/main)
- [ ] All demuxer fuzz targets passing with 0 crashes after 1M iterations
- [ ] At least one downstream consumer (Jalwa, Tazama, Shruti, or Kiran) running on stable tarang
- [ ] docs.rs documentation complete with examples for every public module
- [ ] No `unsafe` blocks without `// SAFETY:` comments
- [ ] `cargo-vet` fully audited (zero exemptions for direct dependencies)
- [x] `cros-libva` patch accepted — 1-line `..Default::default()` fix for libva >= 1.23 VP9 struct compat; upstream 0.0.13 still latest, no release in sight. Patch is stable, tested, minimal. Will remove when upstream > 0.0.13 ships or when AGNOS fork replaces it.
- [ ] SemVer compliance enforced via `cargo-semver-checks` in CI

---

## Post-v1

Longer-term items that don't block any release.

### New codec backends

- [ ] **AV1 decode via rav1e** — if rav1e adds decode support, replace dav1d for pure-Rust AV1
- [ ] **VP8/VP9 pure Rust** — when a viable pure-Rust VP8/VP9 decoder exists, add as alternative to libvpx
- [x] **Drop opus dep** — Opus encode now via shravan (CELT-mode)
- [x] **Drop fdk-aac dep** — AAC encode/decode now via shravan
- [ ] **ALAC decode** — waiting on shravan to add ALAC support (tracked in shravan roadmap)

### Platform support

- [ ] **macOS VideoToolbox** — hardware H.264/H.265 encode/decode via VTCompressionSession/VTDecompressionSession (Phase 4a)
- [ ] **Windows Media Foundation** — hardware H.264/H.265 encode/decode via MFT (Phase 4b)
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
- [ ] **Offline transcription** — hoosh crate provides inference; tarang consumes via `HooshClient`

### Performance

- [ ] **SIMD audio processing** — explicit SIMD for resample, mix, fingerprint inner loops (portable_simd or manual intrinsics)
- [ ] **Zero-copy demux** — `mmap` + `Bytes::from_static` for reading packets without copying from kernel
- [ ] **Lazy metadata parsing** — parse only requested atoms/elements in MP4/MKV instead of full traversal

---

## Non-goals

- **Full ffmpeg replacement** — tarang covers decode, encode, demux, mux, and analysis. It does not aim to replace ffmpeg's filter graph, network protocols, or device capture.
- **Proprietary codec licensing** — no bundling of patent-encumbered codecs (Dolby, DTS). Only FOSS and permissive-licensed backends.
- **GUI** — tarang is a library and CLI. GUI media players (Jalwa) are separate projects.
- **Streaming server** — tarang produces segments/packets. Serving them over HTTP/WebRTC is the consumer's responsibility.
