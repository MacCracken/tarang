# Tarang Roadmap

> **Principle**: FOSS codecs first, proprietary codecs next, wide coverage always in scope.

Completed items are in [CHANGELOG.md](../../CHANGELOG.md).

---

## 0.21.3 — Pre-v1.0 hardening

### P0 — Must-fix for v1.0

- [ ] **`num_samples` → `num_frames` rename** — field name is misleading and has caused bugs; breaking change allowed pre-1.0
- [ ] **`cargo-semver-checks` in CI** — v1.0 gate: enforce SemVer compliance on every PR
- [ ] **Coverage threshold 90%+** — raise CI from 80% to 90%; add tests for convert, scale, effects, loudness, diarize, acoustid
- [ ] **Doc examples for all public modules** — v1.0 gate: `audio::effects`, `audio::loudness`, `video::convert`, `video::scale`, `ai::diarize`, `ai::acoustid`
- [ ] **Fix VA-API `unwrap()` calls** — `vaapi_dec.rs` and `vaapi_enc.rs` `.pop().unwrap()` → proper error handling
- [ ] **Document LOW security items** — enumerate the 6 LOW severity findings with accept/fix decisions

### P1 — Should-fix (quality)

- [ ] **VA-API surface pooling** — reuse pre-allocated surfaces instead of per-frame GPU allocation (encode + decode)
- [ ] **Video convert chroma pre-allocation** — hoist `cr_r`/`cr_g`/`cr_b` Vecs out of per-row loop (~1600 allocs/frame eliminated)
- [ ] **`extract_mono_f32` fast path** — use `bytes_to_f32()` + stride for F32 mono instead of per-byte assembly
- [ ] **Direct YUV scaling** — scale Y/U/V planes independently, skip YUV→RGB→scale→RGB→YUV roundtrip
- [ ] **Effects chain in-place processing** — `AudioEffect::process` takes `AudioBuffer` by value, remove clone storm
- [ ] **`Muxer` trait video support** — add `write_video_packet()` to trait with default error impl
- [ ] **NV12 conversion paths** — add NV12→YUV420p and NV12→RGB24 to `video::convert`

### P2 — Polish

- [ ] **Rename `AudioDecoder` → `AudioCodecInfo`** — vestigial type that isn't a decoder
- [ ] **Verify downstream consumers** — CI job that builds Jalwa/Tazama/Shruti against current tarang
- [ ] **Refresh `cargo-vet` trust entries** — audit for new deps since last review
- [ ] **Remove `cros-libva` patch** — check if > 0.0.13 released, remove `patches/` if so

### Release

- [ ] **Update ai-hwaccel to 0.20.3** — bump dependency version when released
- [ ] **Publish 0.21.3 to crates.io** — `cargo publish --dry-run`, tag, push

---

## v1.0.0 criteria

All of the following must be true before cutting 1.0:

- [ ] Public API reviewed and marked stable (no `#[non_exhaustive]` additions expected)
- [ ] All `Demuxer`/`Muxer`/`AudioEncoder` traits finalized
- [ ] Core types (`AudioBuffer`, `VideoFrame`, `MediaInfo`, `Packet`) frozen
- [ ] 89%+ line coverage (library code, excluding mcp/main)
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
