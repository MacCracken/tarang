# Tarang Roadmap

> **Principle**: FOSS codecs first, proprietary codecs next, wide coverage always in scope.

Completed items are in [CHANGELOG.md](../../CHANGELOG.md).

---

## Waiting on Upstream

- [ ] **VA-API encode pipeline completion** — surface upload, parameter buffers, bitstream readback. Blocked on `cros-codecs` updating its `cros-libva` dependency from `^0.0.12` to `^0.0.13`. Upstream repo has had no commits since March 2025. Workarounds: fork cros-codecs, use `[patch]`, or downgrade to cros-libva 0.0.12. *(added 2026-03-16, audited 2026-03-19)*

- [ ] **rav1e `paste` dependency** — PR #3442 merged upstream (paste → pastey), but no release since 0.8.1 (September 2025). Resolves automatically when rav1e cuts 0.8.2 or 0.9.0. *(added 2026-03-16, audited 2026-03-19)*

---

## Pre-v1 (0.20–0.x)

### API stabilization

- [ ] **`#[non_exhaustive]` on all public enums** — `AudioCodec`, `VideoCodec`, `ContainerFormat`, `SampleFormat`, `PixelFormat`, `StreamInfo`, `TarangError` — prevents downstream breakage when adding variants
- [ ] **Review public API surface** — audit every `pub fn`, `pub struct`, `pub enum` for consistency; ensure all public items have doc comments; hide internal helpers behind `pub(crate)`
- [ ] **Consistent error types** — evaluate whether `TarangError` variants cover all failure modes cleanly; consider module-specific error enums that convert into `TarangError`
- [ ] **Trait stability** — finalize `Demuxer`, `Muxer`, `AudioEncoder`, `AudioOutput` traits; document contracts, lifetimes, and threading guarantees
- [ ] **Builder patterns** — add builders for complex configs (`EncoderConfig`, `HooshConfig`, `FingerprintConfig`, `SceneDetectionConfig`) instead of exposing struct fields directly

### Codec gaps

- [ ] **AAC decoding via fdk-aac** — symphonia handles AAC but fdk-aac may offer better quality for some profiles; evaluate as optional backend
- [ ] **HEVC/H.265 decoding** — VA-API scaffolding exists for encode; add decode path (either via VA-API or a pure-Rust decoder when available)
- [ ] **WebM muxer improvements** — ensure Opus-in-WebM and VP9-in-WebM roundtrip correctly; add DASH segmentation support
- [ ] **Subtitle stream support** — parse subtitle tracks from MKV/MP4; expose as `StreamInfo::Subtitle` with text extraction
- [ ] **ID3/Vorbis comment metadata** — extract full tag metadata from MP3, FLAC, OGG containers (currently only basic fields)

### Demuxer/muxer hardening

- [ ] **Fuzz testing** — `cargo-fuzz` targets for MP4, MKV, OGG, WAV demuxers with malformed input
- [ ] **64-bit MP4 muxing** — full `co64` and extended `mdat` box support for files > 4GB
- [ ] **Fragmented MP4 (fMP4)** — `moof`/`mdat` segment parsing for streaming/DASH
- [ ] **MP4 edit lists** — `elst` box parsing for correct timestamp mapping
- [ ] **MKV chapters** — parse chapter elements for navigation
- [ ] **OGG chaining** — multiple logical streams concatenated (podcast chapters)

### Audio pipeline

- [ ] **Streaming decode API** — frame-by-frame decode without `decode_all()` loading entire file; callback or async stream interface
- [ ] **Sample format conversion** — explicit `AudioBuffer::convert_to(SampleFormat)` instead of relying on encoder-internal conversion
- [ ] **Gapless playback** — encoder delay / padding metadata for seamless track transitions
- [ ] **Loudness normalization** — EBU R128 / ReplayGain analysis and adjustment
- [ ] **Audio effects pipeline** — EQ, compressor, limiter as composable transforms

### Video pipeline

- [ ] **Frame format conversion** — YUV420p ↔ RGB24 ↔ NV12 as explicit operations (currently scattered in thumbnail/encoder code)
- [ ] **Scaling/resize** — bilinear/bicubic/Lanczos frame scaling as a standalone operation
- [ ] **Hardware decode via VA-API** — wire dav1d fallback to VA-API for H.264/HEVC when hardware available

### AI features

- [ ] **Offline transcription** — bundle a small Whisper model (tiny/base) for local inference without hoosh dependency
- [ ] **Speaker diarization** — who-spoke-when segmentation for podcast/meeting analysis
- [ ] **Music fingerprinting** — AcoustID-compatible fingerprinting for music identification
- [ ] **Content-based thumbnails** — face detection or saliency-based frame selection instead of variance scoring

### Testing & CI

- [ ] **Fuzz targets in CI** — run `cargo-fuzz` in nightly CI job for all demuxer parsers
- [ ] **Benchmark regression CI** — track criterion numbers across releases; fail on >10% regression
- [ ] **Integration test suite** — end-to-end tests with real media files (small test fixtures in repo or downloaded in CI)
- [ ] **Cross-platform CI** — test on macOS (no VA-API, no PipeWire) and verify feature-gated compilation

### Documentation

- [ ] **docs.rs examples** — add `examples/` directory with runnable programs (probe, transcode, fingerprint)
- [ ] **Migration guide** — document breaking changes between minor versions for downstream consumers
- [ ] **Troubleshooting guide** — common issues: missing system deps, feature flag confusion, FFI build errors
- [ ] **Performance tuning guide** — when to use sinc vs linear resample, chunk size selection, buffer reuse patterns

### Release prep

- [ ] **Publish 0.19.3 to crates.io** — verify `cargo publish --dry-run` passes; tag and push
- [ ] **Switch to SemVer strictly** — 0.x allows breaking changes; document policy in CONTRIBUTING.md
- [ ] **Set up codecov** — configure coverage upload and badge in README
- [ ] **Create GitHub release workflow** — automated changelog generation from git tags

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
