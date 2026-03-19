# Changelog

## 0.19.3

Single-crate restructure, crates.io publishing, security audit, supply-chain hardening.

### Crate restructure
- Flattened workspace into single `tarang` crate — 5 sub-crates become modules (`core`, `demux`, `audio`, `video`, `ai`)
- Single `cargo publish` / `cargo install tarang` — no multi-crate orchestration
- Feature flags consolidated: `full`, `dav1d`, `vpx`, `openh264`, `rav1e`, `vaapi`, `pipewire`, `opus-enc`, `aac-enc`
- Package metadata: homepage, repository, readme, keywords, categories, MSRV 1.89

### crates.io publishing
- Release workflow matches ai-hwaccel: CI gate → version verification → test → build → publish → GitHub release
- Version consistency check: VERSION file, Cargo.toml, and git tag must match
- `cargo publish --dry-run` passes clean

### Security audit (5 areas, 40+ issues fixed)

#### FLAC encoder
- Off-by-one in fixed prediction: `num_frames <= order` → `num_frames <= order + 1` (prevented panic on slice bounds)
- LPC prediction: saturating arithmetic for i64 coefficient × sample multiplication
- Fixed prediction orders 2-4: saturating arithmetic for all intermediate multiplications
- Channel count validation: reject >8 channels (FLAC spec limit)

#### MP4 muxer
- Empty sample_sizes panic: guard in `build_stsz` returns valid zero-count box
- Size overflow in `write_box`: checked arithmetic, error on >4GB box data
- stco offset overflow: `build_stco` returns `Result`, errors if offset > u32::MAX
- Duration overflow: saturating multiplication capped at u32::MAX

#### Audio resampler
- Sinc interpolation: bounds check before casting i64 index to usize (prevented negative index UB)
- Window size validation: reject 0 or >1024
- Precision: keep interpolation fraction as f64 throughout (was truncating to f32)

#### Core types and utilities
- `yuv420p_frame_size`: checked arithmetic for width × height multiplication
- `extract_mono_f32`: zero-channels guard prevents division by zero
- `audio_utils`: checked multiplication for interleaved offset calculations
- PCM scaling: fixed inconsistent i16 divisor (32768.0 → 32767.0, matching `I16_SCALE`)
- `video_utils`: checked multiplication for frame dimensions, stride validation for RGB/RGBA
- `f32_to_bytes`: checked multiplication before unsafe slice cast

### Supply-chain hardening
- `cargo-vet` initialized with Mozilla audit imports, 32 trusted publisher audits
- `cargo-deny` config: license allowlist (GPL-3.0), vulnerability deny, source restrictions
- Both added to CI pipeline (vet + deny jobs gate release build)

### Documentation (matching ai-hwaccel strategy)
- `CONTRIBUTING.md`: dev workflow, system deps, feature flags, adding codecs, code style
- `CODE_OF_CONDUCT.md`: Contributor Covenant v2.1
- `SECURITY.md`: threat surface (media parsing, FFI, AI APIs, MCP)
- `Makefile`: check/fmt/clippy/test/audit/deny/build/doc/clean targets
- `rust-toolchain.toml`: stable channel with rustfmt + clippy
- ADRs: feature-flags-per-codec, semver-versioning
- `docs/development/threat-model.md`: media parsing, FFI safety, network, supply chain

### Previous work (from 2026.3.19)

#### FLAC encoding
- Full pure-Rust FLAC encoder: fixed-predictor selection, residual Rice coding, CRC checksums
- Linear LPC prediction (Levinson-Durbin): autocorrelation, coefficient quantization, orders 1-8
- Configurable compression level, block size, and bit depth (16/24)
- STREAMINFO, PADDING, and SEEKTABLE metadata block generation

#### EBML parser
- Generic EBML element parser for MKV/WebM containers
- Variable-width integer (VINT) decoding, element tree traversal

#### MCP server extraction
- Extracted from monolithic `main.rs` into `src/mcp/{mod,tools}.rs`

#### AI module
- `audio_utils.rs` / `video_utils.rs`: shared preprocessing helpers
- Daimon client: LLM-powered media analysis
- Scene detector: improved transition thresholds and debouncing
- Thumbnail generator: better frame scoring heuristics

#### Codec & demuxer
- MP4: improved box parsing, MKV: SimpleBlock hardening, OGG: seek/CRC
- rav1e: expanded config, dav1d: stride handling, AAC/Opus: truncation guards

#### Audio pipeline
- Resampler: sinc interpolation, SIMD-friendly linear path
- PipeWire: runtime safety, Mixer: channel handling
- Release workflow gates on CI + version verification before publish and binary packaging

## 2026.3.16-1

F3 AI features — all four items complete. Security audit, bug fixes, and test coverage hardening.

### Scene detection (`scene.rs`)
- SceneDetector: stateful feed-frame API with hard-cut and gradual-transition detection
- Chi-squared histogram distance on luminance channel
- Rolling standard deviation for fade/dissolve detection
- Min-scene-duration debouncing; supports YUV420p and RGB24
- 8 tests

### Thumbnail generation (`thumbnail.rs`)
- ThumbnailGenerator: variance-based frame scoring with scene-boundary preference
- YUV420p→RGB24 conversion (BT.601), bilinear resize via `image` crate
- JPEG and PNG encoding with configurable quality
- Aspect-ratio-preserving resize; rejects solid-color frames
- 8 tests

### Transcription routing (`transcribe.rs`)
- HooshClient: async multipart HTTP client for Whisper endpoint
- Audio preprocessing: stereo→mono downmix (F32 channel averaging)
- In-memory WAV encoding (PCM16) from any AudioBuffer format
- WhisperModel enum, HooshConfig with timeout/API key
- 7 tests

### Audio fingerprinting (`fingerprint.rs`)
- Chromaprint-style fingerprinting: FFT → chroma bands → differential hashing
- `compute_fingerprint` / `fingerprint_match` with sliding-window Hamming distance
- Supports F32 and I16 input; configurable FFT/hop/bands
- 8 tests

### Dependencies added
- `rustfft = "6"` (pure Rust FFT for fingerprinting)
- `image = "0.25"` (JPEG/PNG encoding for thumbnails)
- `reqwest` multipart feature (for hoosh client)

### Security & correctness fixes (2 audit rounds, 18 HIGH + 8 MEDIUM)

#### HIGH severity
- **tarang-core**: MP3 magic byte detection panicked on short buffers — added `bytes.len() >= 2` guard
- **tarang-audio**: `bytes_to_f32` panicked via `assert!()` in 7 files — replaced with graceful empty-slice return
- **tarang-audio/pw.rs**: Unsafe `from_raw_parts` without alignment check — added validation and error return
- **tarang-video/vaapi_enc.rs**: `frames_encoded` counter incremented before error return — removed
- **tarang-video/rav1e_enc.rs**: No frame dimension or data size validation — added both
- **tarang-ai/daimon.rs**: Unchecked `["choices"][0]` JSON indexing — replaced with safe `.get()` chain
- **tarang-ai/daimon.rs**: RAG query silently swallowed HTTP errors — added warning log
- **tarang-ai/thumbnail.rs**: `partial_cmp().unwrap()` on f64 (NaN panic) — replaced with `total_cmp()`
- **src/main.rs**: 7 MCP tool handlers silently accepted empty paths — extracted `require_path()` with error return
- **src/main.rs**: `let _ =` swallowed async errors — now logs warnings
- **tarang-demux/mkv.rs**: `size - header_size` underflow on malformed SimpleBlock — added bounds check
- **tarang-audio/encode_opus.rs**: Unchecked slice on truncated buffers — added bounds guard
- **tarang-demux/mp4.rs**: Size-0 boxes set `data_size = u64::MAX` (OOM) — capped at 4 GB
- **tarang-video/dav1d_dec.rs**: Plane slicing trusted FFI stride — added bounds validation on all 3 planes

#### MEDIUM severity
- **tarang-core**: Added `Hash` derive to `SampleFormat` and `PixelFormat`
- **tarang-video/rav1e_enc.rs**: `bitrate_bps as i32` overflow — clamped to `i32::MAX`
- **tarang-audio/probe.rs**: Bitrate calculation overflowed u32 — switched to `checked_mul()`
- **tarang-demux/wav.rs**: Same bitrate overflow — switched to `checked_mul()`
- **tarang-demux/mux.rs**: WAV muxer RIFF size overflowed at 4 GB — switched to `saturating_add()`
- **tarang-ai/transcribe.rs**: Stereo→mono downmix divided by total channels even when truncated — now divides by actual count
- **tarang-audio/pw.rs**: Thread name `jalwa-pipewire` → `tarang-pipewire`

### Test coverage: 303 tests (was 200)
- **tarang-core**: +12 (error variants, Display completeness, serialization, edge-case MediaInfo)
- **tarang-ai**: +54 (boundary conditions, edge cases, error paths, serde roundtrips)
- **tarang-audio**: +25 (mix edge cases, decode helpers, resample validation, FLAC encoding, bytemuck safety)
- **tarang-video**: +7 (status transitions, default dimensions, timestamps, config validation)
- **tarang-demux**: +8 (all format detection variants, keyframe packets)

### Refactoring (30 backlog items cleared)
- Extracted `sample.rs` shared module — `bytes_to_f32`, `f32_to_bytes`, PCM scaling constants, test helpers; removed 12 duplicate copies across 7 files
- `yuv420p_frame_size()` and `validate_video_dimensions()` in tarang-core — used by all video encoders
- PipeWire rearchitecture: lock-free SPSC ring buffer (AtomicUsize), condvar ready signal (replaced 50ms sleep), deadline-based flush (replaced fixed sleep loop)
- OGG CRC-32 validation (demuxer) + CRC generation (muxer); bisection seek (O(log n))
- OGG muxer: randomized serial numbers, codec validation at construction
- Probe: auto-detects container format from symphonia codec type
- WebM vs MKV detection via EBML DocType parsing
- `VideoDecoder` wired to real backends (dav1d, openh264, libvpx) with unified send/receive API, pending-frame queue, flush drain
- `Arc<VideoFrame>` in ThumbnailGenerator (avoids cloning megabyte frame data)
- O(1) `Bytes::clone` in resample/mix no-op paths (was deep copy)
- `Copy` derive on `OutputConfig` and `EncoderConfig`
- Bounds-checks and error propagation across OGG/MP4/MKV demuxers, dav1d decoder planes, rav1e even dimensions, VPX bitrate floor, daimon endpoint validation

### Engineering
- 310 total tests (was 200 before audit)
- All 30 backlog items resolved (1 remains: VA-API encode, blocked on upstream)
- Downstream consumers (Jalwa, Tazama, Shruti) marked as integrated in roadmap

## 2026.3.16e

Encoder API normalization, security fixes, libvpx-sys migration, test coverage.

### Encoder API normalization
- All encoders now use `bitrate_bps: u32` (bits per second) — was kbps in vpx, unnamed in rav1e
- All encoders now use `frame_rate_num: u32, frame_rate_den: u32` — was f32 in openh264/vaapi, u64 in rav1e
- All speed presets now use `u32` — was i32 in vpx, usize in rav1e

### Security fixes
- Upgraded openh264 0.6 → 0.9 (fixes RUSTSEC-2025-0008 heap overflow in openh264-sys2)
- Migrated libvpx-sys 1.4 → env-libvpx-sys 5.1 (eliminates RUSTSEC-2023-0018 remove_dir_all TOCTOU, RUSTSEC-2018-0017 tempdir unmaintained)
- `cargo audit` now passes with 0 vulnerabilities

### libvpx-sys → env-libvpx-sys migration
- Bindings now generated from system headers via bindgen — struct layouts always match
- VPX_ENCODER/DECODER_ABI_VERSION exported directly — build.rs probe eliminated
- Encoder ABI mismatch with libvpx >= 1.14 resolved — all VPX encoder tests now pass
- MaybeUninit for encoder config (new bindgen types can't be zero-initialized)
- Proper Rust enums for error codes, image formats, packet kinds

### Clippy cleanup (workspace-wide)
- Fixed all clippy warnings across tarang-audio, tarang-demux, tarang-video
- Removed dead code (unused struct fields), replaced manual patterns with idiomatic Rust

### Test coverage: 155 base + 62 feature-gated = 217 total tests
- VP8/VP9 encode-decode roundtrip verified (previously blocked by ABI mismatch)
- All VPX encoder tests un-ignored and passing

## 2026.3.16d

Test coverage + VA-API encode scaffolding.

### Tests added (33 new tests across codec modules)
- openh264_enc: 8 tests (creation, validation, encode, roundtrip errors)
- openh264_dec: 3 tests (creation, empty input, encode-decode roundtrip)
- vpx_enc: 8 tests (creation, validation, encode, flush — 5 ignored pending libvpx-sys upgrade)
- vpx_dec: 4 tests (creation, unsupported codec, invalid data, encode-decode roundtrip)
- vaapi_enc: 6 tests (profile mapping, config, validation, HW creation)
- vaapi_probe: 16 tests (from previous — profile/entrypoint mapping, report queries)

### VA-API encode
- VaapiEncoder scaffolding: display open, profile/entrypoint negotiation, dimension validation
- Supports H.264 and HEVC encode entrypoint detection (EncSlice + EncSliceLP)
- Auto-discovers DRM render nodes with TARANG_VAAPI_DEVICE override
- Full encode pipeline (surface upload, parameter buffers, bitstream readback) pending cros-codecs version alignment

### Engineering
- Identified libvpx-sys 1.4 ABI mismatch with system libvpx >= 1.14 (encoder config struct layout changed)
- Added backlog items for libvpx-sys upgrade and VA-API encode pipeline completion

## 2026.3.16c

VA-API hardware acceleration detection.

- VA-API probe via cros-libva: detects GPU capabilities (decode/encode) for H.264, HEVC, VP8, VP9, AV1
- `vaapi` feature flag, `HwAccelReport` type with `can_decode()`/`can_encode()` queries
- DRM render node auto-discovery (`/dev/dri/renderD*`) with `TARANG_VAAPI_DEVICE` override
- `DecoderBackend::Vaapi` variant added
- VDPAU dropped from roadmap (Mesa removed support from all open-source drivers)
- 16 new unit tests for profile mapping, entrypoint mapping, report queries
- `HwAccelError` variant added to `TarangError`

## 2026.3.16b

Video codec bindings — V3, F1, F2 complete.

### New features
- VP8/VP9 encoding via libvpx FFI (`vpx-enc` feature flag)
- H.264 decoding via openh264 (`openh264` feature flag, Cisco BSD-2-Clause)
- H.264 encoding via openh264 (`openh264-enc` feature flag)
- `full` convenience feature flag enables all codec backends
- VPX ABI version auto-detection via build.rs probe (with env-var override for cross-compilation)

### Safety & correctness (3 rounds of code review)
- RAII guard (`VpxImageGuard`) for vpx image lifetime — panic-safe
- Bounds checks on all frame data before unsafe plane copies
- Signed (`isize`) stride arithmetic in all vpx FFI — handles negative strides
- Pixel format validation: dav1d rejects non-I420, vpx_dec rejects non-I420, openh264_enc requires YUV420p
- Ceiling division for chroma dimensions in all decoders (correct for odd sizes)
- Frame dimension and data size validation in all encoders
- Zero-dimension and zero-framerate rejection
- `vpx_codec_control_` return value checked with proper cleanup on failure
- `data.len() > u32::MAX` guard in vpx decoder
- `DecoderConfig::for_codec()` now validates that the required feature is enabled
- `supported_codecs()` only lists compiled-in backends
- `unsafe impl Send` for VpxEncoder/VpxDecoder (single-owner contexts)
- Removed redundant `initialized` field from vpx types
- openh264 decoder: `flush()` method drains buffered frames with correct timestamps
- openh264 encoder: zero-copy `YUVSlices` instead of `YUVBuffer::from_vec`
- openh264 encoder: uses `encode_at()` with frame timestamps
- vpx encoder: uses frame timestamps for PTS
- dav1d decoder: clamps negative timestamps to 0
- dav1d decoder: documents timestamp unit contract
- build.rs: `rerun-if-changed` directives, `$CC` respect, both-or-neither env-var check
- Fixed dav1d stride type mismatch (`u32` → `usize` cast for dav1d 0.11)
- Fixed vpx crate name (`vpx_sys` not `libvpx_sys`)

## 2026.3.16

Initial scaffolding.

- Core types: audio/video codecs, container formats, media buffers, pipeline primitives
- Container demuxing: WAV demuxer (pure Rust), format detection via magic bytes
- Audio decoding: symphonia integration (MP3, FLAC, WAV, OGG Vorbis, AAC, ALAC)
- Video decoding: decoder framework with dav1d/openh264/libvpx backend stubs
- AI features: content classification, quality scoring, codec recommendations, transcription prep
- CLI: probe, analyze, codecs, mcp subcommands
- MCP server: 5 tools (tarang_probe, tarang_analyze, tarang_codecs, tarang_transcribe, tarang_formats)
- CI/CD: GitHub Actions (check, test, clippy, fmt, multi-arch release)
