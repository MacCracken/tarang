# Tarang Roadmap

> **Principle**: FOSS codecs first, proprietary codecs next, wide coverage always in scope.

Completed items are in [CHANGELOG.md](../../CHANGELOG.md).

---

## Post-v1

Longer-term items that don't block any release.

### Pure Rust codec backends (drop remaining C FFI deps)

- [ ] **AV1 decode** — replace dav1d (C FFI); reference impl via vidya
- [ ] **AV1 encode** — replace rav1e or complement with AGNOS encoder; reference impl via vidya
- [ ] **H.264 decode/encode** — replace openh264 (C FFI); reference impl via vidya
- [ ] **H.265 decode** — replace libde265 (C FFI); reference impl via vidya
- [ ] **VP8/VP9 decode/encode** — replace libvpx (C FFI); reference impl via vidya

### Patches to remove

- [ ] **cros-libva patch** — 1-line `..Default::default()` fix for libva >= 1.23 VP9 struct compat; remove when upstream > 0.0.13 ships or AGNOS fork replaces it
- [ ] **env-libvpx-sys patch** — bindgen 0.72 bump + fallback bindings; remove when replaced by chitrsys

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
