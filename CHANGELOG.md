# Changelog

## 0.20.3

ai-hwaccel integration, hardware-aware codec selection, P1 fixes.

### H.265/HEVC software decoding
- `LibDe265Decoder` — software H.265 decode via libde265 (`h265-decode` feature)
- **WARNING: LGPL-3.0** — deliberately excluded from `full` feature; opt-in only
- Push-based API: `push_data()`/`push_nal()` → `decode()` → `next_frame()`
- Multi-threaded slice decoding via `start_threads()`
- Outputs YUV420p `VideoFrame` with stride-aware plane extraction
- `DecoderConfig::for_codec(H265)` now succeeds when feature enabled
- Removal instructions in module header and Cargo.toml comments
- 4 new tests

### API stabilization — error types
- Added `EncodeError`, `MuxError`, `ConfigError` variants to `TarangError`
- Migrated ~40 `Pipeline` misuses: muxer state errors → `MuxError`, encoder errors → `EncodeError`, validation errors → `ConfigError`
- `Pipeline` reduced from catch-all to genuine pipeline state issues only

### AI features
- **Speaker diarization**: `ai::diarize` module — energy-based VAD + spectral clustering for who-spoke-when segmentation; `diarize()` returns `Vec<SpeakerSegment>` with timing and speaker IDs; 8 tests
- **AcoustID fingerprinting**: `ai::acoustid` module — Chromaprint-compatible compressed fingerprints for music identification; `compute_acoustid()` returns base64-encoded fingerprint string; 7 tests
- **Content-based thumbnails**: `ThumbnailStrategy::ContentBased` — saliency scoring with edge density, color diversity, skin tone detection, center weighting; replaces variance-only scoring as default; 3 tests

### Documentation
- **Examples**: `examples/probe.rs` (media info), `examples/transcode.rs` (decode→resample→encode→mux), `examples/fingerprint.rs` (audio fingerprint + comparison)
- **Migration guide**: `docs/development/migration-guide.md` — 0.19.3→0.20.3 breaking changes and new features
- **Troubleshooting guide**: `docs/development/troubleshooting.md` — system deps, feature flags, VA-API, runtime errors
- **Performance guide**: `docs/development/performance-guide.md` — resampling, encoding, hardware accel, memory tuning
- **SemVer policy**: documented in CONTRIBUTING.md, enforced via cargo-semver-checks in CI

### Testing & CI
- Fuzz targets in CI workflow (nightly, 60s per demuxer)
- Benchmark job with criterion
- macOS cross-platform CI (default features, no VA-API/PipeWire)
- Integration test roundtrip (WAV mux→demux)

### Audio pipeline
- **Streaming decode API**: already complete via `FileDecoder::next_buffer()` (pull-based iterator)
- **Sample format conversion**: `AudioBuffer::convert_to(SampleFormat)` — F32 → I16, I32, F64
- **Gapless playback**: `AudioEncoder::delay_samples()` / `padding_samples()` trait methods for encoder delay/padding metadata
- **Loudness normalization**: `audio::loudness` module — `measure_loudness()` (simplified BS.1770 LUFS), `apply_gain()`, `normalize_loudness()` with target LUFS; 6 tests
- **Audio effects pipeline**: `audio::effects` module — `AudioEffect` trait, `EffectChain` for composable transforms, built-in `Gain`, `HighPassFilter`, `Compressor`; 7 tests

### Video pipeline
- **Frame format conversion**: `video::convert` module — centralized `convert_pixel_format()`, `VideoFrame::convert_to()`, YUV420p↔RGB24↔NV12 conversions
- **Scaling/resize**: `video::scale` module — `scale_frame()` with `ScaleFilter` (Nearest, Bilinear, Lanczos3) for RGB24 and YUV420p frames
- **VA-API hardware decode**: `VaapiDecoder` — GPU-accelerated decode for H.264/H.265/VP9/AV1/VP8
  - Full pipeline: display, config (VLD entrypoint), context, surfaces, SliceData submission
  - NV12→YUV420p readback via Image API
  - Wired into `BackendInner::Vaapi` and `VideoDecoder::send_packet()`
  - 3 tests (profile mapping, dimension validation, hardware creation)

### Fuzz testing
- `cargo-fuzz` targets for all 4 demuxers: `fuzz_wav`, `fuzz_mp4`, `fuzz_mkv`, `fuzz_ogg`
- Each target: probe → read packets → seek on arbitrary input
- **Bug found and fixed**: MP4 `data_offset + data_size` arithmetic overflow on malformed boxes — replaced with `saturating_add` across 12 sites
- Results: WAV 15.8M runs, MP4 11.4M runs, MKV 6.3M runs, OGG 6.9M runs — 0 crashes after fix

### Demuxer/muxer hardening
- **64-bit MP4 muxing**: MP4 muxer auto-switches from `stco` to `co64` for files > 4GB
- **MKV chapters**: `MkvDemuxer` parses Chapters/EditionEntry/ChapterAtom EBML elements, extracts time and title; `chapters()` accessor
- **EBML writer fix**: `write_uint()` now handles values > 0xFFFFFFFF (was truncating to 4 bytes)
- **Mp4Muxer video track support**: `Mp4Muxer::new_with_video()` for audio+video MP4 files
  - `write_video_packet()` for video samples to track 2
  - Video stsd with codec-specific box types: avc1 (H.264), hev1 (H.265), vp09 (VP9), av01 (AV1)
  - vmhd, video tkhd (with dimensions), stss (sync samples), 90kHz timescale
- **Fragmented MP4 / DASH muxer**: `FragmentedMp4Muxer` for DASH/HLS streaming
  - `write_init_segment()` generates `ftyp` + `moov` with `mvex`/`trex`
  - `add_audio_sample()` / `add_video_sample()` + `flush_fragment()` generates `moof` + `mdat` pairs
  - `mfhd` (sequence numbers), `traf`/`tfhd`/`trun` (track fragments with per-sample sizes)
  - 5 new tests: init segment, fragment structure, multiple fragments, empty fragment, error handling
- **fMP4 demuxing**: `Mp4Demuxer` detects and parses `moof` boxes with `mfhd`/`traf`/`tfhd`/`trun`
- **MP4 edit lists**: `parse_edts()`/`parse_elst()` — parses `edts`/`elst` boxes, stores edit list entries per track (version 0 and 1)
- **OGG chaining**: `next_packet()` detects mid-file BOS pages and registers new logical streams dynamically
- 13 new tests total for demux/mux hardening

### Bug fixes (found via shruti benchmarks)
- **Linear resampler off-by-one on stereo buffers** — `resample()` and `resample_sinc()` now derive frame count from actual data length (`src.len() / channels`) instead of trusting `num_samples` field, preventing index-out-of-bounds panic when `num_samples` is set to total interleaved samples rather than frames
- **`mix_channels` stereo→mono validation** — same fix: derives frame count from data length instead of `num_samples`, preventing false "source buffer too small" errors on stereo buffers
- 3 regression tests added: `resample_stereo_interleaved_num_samples`, `resample_stereo_downsample`, `mix_stereo_to_mono_interleaved_num_samples`

### P1 fixes

#### VA-API encode pipeline (fully wired)
- Complete H.264 encode via VA-API: surface creation, NV12 upload, SPS/PPS/slice parameter buffers, encode submission, bitstream readback
- `VaapiEncoder::encode()` now produces real H.264 bitstream (was stub error)
- YUV420p → NV12 conversion for VA-API surface upload
- IDR-only encoding (P/B frames deferred to future release)
- Patched cros-libva 0.0.13 for libva >= 1.23 compatibility (VP9 `seg_id_block_size` field)
- `[patch.crates-io]` in Cargo.toml for local cros-libva fix

#### H.265 decode path
- H.265 decode now available via VA-API hardware acceleration (confirmed on AMD Renoir)
- `DecoderConfig::for_codec_auto()` routes H.265 to VA-API when hardware supports it
- Updated error message in `for_codec()` to guide users toward `hwaccel` feature
- Documented H.265 hardware-only status in lib.rs codec table

#### AAC encode documented
- Documented fdk-aac FFI requirement in lib.rs codec table and encode_aac.rs module docs
- Listed system package names (libfdk-aac-dev, fdk-aac-devel, fdk-aac)
- Documented Opus and FLAC as pure-Rust alternatives

#### rav1e/paste unblocked
- rav1e 0.8 compiles and works; `paste` advisory (RUSTSEC-2024-0436) suppressed in deny.toml
- Expanded deny.toml documentation explaining the advisory scope and removal criteria

### Codec gaps

#### AAC decoding via fdk-aac
- `FdkAacDecoder` — optional fdk-aac-backed AAC decoder (`aac-dec` feature)
- Supports raw AAC (MP4) and ADTS transport formats
- `configure()` for AudioSpecificConfig from MP4 esds box
- `decode()` returns F32 AudioBuffer with auto-detected sample rate/channels
- 4 new tests

#### Subtitle stream parsing
- MKV demuxer: parses subtitle tracks (TrackType 0x11), extracts language from EBML
- MP4 demuxer: detects subtitle handler types (`sbtl`, `text`, `subt`)
- Both emit `StreamInfo::Subtitle { language }` in probe results
- Refactored MP4 `parse_hdlr()` to return `Mp4TrackType` enum (Audio/Video/Subtitle/Other)

#### WebM muxer improvements
- `MkvMuxer::new_webm()` — audio+video muxing for WebM (Opus + VP9/VP8/AV1)
- `write_video_packet()` — write video frames to track 2
- Video track header: codec ID, PixelWidth, PixelHeight in EBML
- Full codec ID mapping: V_VP8, V_VP9, V_AV1, V_MPEG4/ISO/AVC, V_MPEGH/ISO/HEVC
- `VideoMuxConfig` — video track configuration struct
- 2 new tests: WebM A/V roundtrip, error on video packet without video track

### API stabilization
- Doc comments on all public struct fields: `AudioStreamInfo`, `VideoStreamInfo`, `AudioBuffer`, `VideoFrame`, `DecoderConfig`, `EncoderConfig`, `MuxConfig`
- Doc comments on AI types: `MediaAnalysis` (score ranges), `TranscriptionRequest/Result/Segment` (units, ranges)
- `Muxer` trait: documented state machine contract (write_header → write_packet → finalize)
- `EncoderConfig::builder()`: builder pattern for audio encoder configuration
- AGNOS ecosystem integration: Jalwa, Tazama, Shruti marked done

### ID3/Vorbis comment metadata
- `MediaInfo.metadata: HashMap<String, String>` for arbitrary tags
- Symphonia metadata extraction: title, artist, album, genre, tracknumber, date, composer, album_artist, comment
- Populates `title`/`artist`/`album` convenience fields from extracted tags
- 4 new tests: empty metadata, metadata fields, tag extraction, empty value skipping

### ai-hwaccel integration
- `hwaccel` feature flag with `ai-hwaccel` 0.19 dependency (Vulkan backend)
- `HardwareReport` / `AcceleratorInfo`: unified GPU/NPU/TPU detection via `probe_hardware()`
- `CodecCapabilities`: maps hardware detection + VA-API probing to tarang codec decode/encode matrix
- `probe_codec_capabilities()`: combines feature flags, VA-API, and ai-hwaccel into one report
- `recommend_decode_backend()` / `recommend_encode_backend()`: hardware-preferred backend selection
- `DecoderConfig::for_codec_auto()`: runtime hardware-aware decoder config (VA-API → software fallback)
- `supported_codecs_with_hw()`: includes VA-API hardware backends in codec listing
- `tarang probe --hw` CLI: shows accelerators, codec capabilities, and recommended device
- 13 new tests for capability matching, recommendation logic, and display formatting

## 0.19.3

Single-crate restructure, crates.io publishing, comprehensive security hardening, supply-chain audit.

### Zero-copy optimizations
- `f32_vec_into_bytes()`: zero-copy Vec<f32> → Vec<u8> reinterpretation (no memcpy)
- Resample (linear + sinc): ownership transfer instead of Bytes::copy_from_slice
- Mix: same ownership transfer pattern
- decode_all(): ownership transfer for accumulated f32 data

### Error allocation reduction
- `TarangError` variants now use `Cow<'static, str>` instead of `String`
- Static error messages use `Cow::Borrowed` (zero allocation)
- Dynamic messages use `Cow::Owned` (allocates only when needed)
- ~250 allocation sites across 30 files updated

### Crate documentation
- Comprehensive lib.rs doc comment (191 lines) for docs.rs/crates.io
- Introduction, supported formats tables, quick start example
- Step-by-step guide (probe → decode → process → analyze)
- Feature flags table, CLI usage, MSRV, versioning
- Supply-chain: regenerated cargo-vet exemptions for criterion + aws-lc deps

### Performance optimizations
- FLAC BitWriter: pre-allocate 8KB per frame (eliminated incremental Vec growth)
- MP4/OGG/MKV demuxers: reuse packet/page buffers across reads (eliminated per-packet allocation)
- Sinc resampler: tiled LUT computation caps memory at ~2MB regardless of input size (was 197MB+ for long audio)
- PCM encoder: replaced `unreachable!()` with proper error return

### Code deduplication
- Extracted shared `copy_yuv420p_from_planes()` helper in video module (~150 lines removed from dav1d, vpx, openh264 decoders)
- Made `ebml` module private (internal to demux only)

### Benchmarks (criterion)
- `benches/audio.rs`: resample (linear, sinc, downsample), mix, FLAC encode, PCM encode
- `benches/demux.rs`: WAV probe + read, MP4 probe + read (1000 packets)
- `benches/ai.rs`: fingerprint (1s, 10s), scene detection (SD, 1080p), luminance variance, media analysis
- `make bench` target added to Makefile

### Chunked transcription
- Configurable `max_wav_bytes` on `HooshConfig` (default 100MB, capped at 1GB)
- Audio exceeding the limit is automatically chunked into `chunk_duration_secs` segments (default 5 min)
- Chunks transcribed sequentially, segment timestamps shifted by chunk offset, results merged
- `HooshConfig::with_max_wav_bytes()` builder for custom limits

### Second hardening pass (435 tests, was 413)

#### Demux — correctness and edge cases
- MP4: skip stsc entries with samples_per_chunk=0; skip stts entries with delta=0 in seek
- OGG: cap page body at 65535 bytes; clamp negative granule positions to 0
- WAV: validate bytes_per_sample > 0 in next_packet; validate seek offset against data_size
- MKV: validate cluster_offset in seek (reject > u64::MAX/2)

#### Audio — safety and buffer reuse
- Decoder: validate sample_rate > 0 and channel count <= u16::MAX from symphonia
- Mixer: bounds validation on source buffer size at entry
- sample.rs: runtime alignment check in bytes_to_f32 (was debug_assert only)
- Opus encoder: reuse output buffer across frames (was allocating 4KB per frame)
- AAC encoder: pre-allocate i16 conversion buffer on construction

#### Video — FFI bounds and limits
- dav1d: stride validation (>= width), explicit negative timestamp clamping comment
- openh264 encoder: checked arithmetic on y_size + chroma calculations
- Stub decoder: cap frame dimensions at 8192
- Decoder framework: documented pointer lifetime contract for VPX

#### AI — input validation
- Thumbnail: cap dimensions at 16384, default 0→source
- Scene detector: reject 0×0 frames in feed_frame
- Daimon: 10MB response body limit on all endpoints, validate timeout > 0
- Transcription: configurable max WAV size, chunked ingestion for large audio

#### MCP — protocol
- BufWriter for stdout responses (was unbuffered println)

### First hardening pass (100+ issues audited across all modules)

#### Demux — input validation and resource limits
- WAV: reject files with zero channels or zero bits_per_sample (prevented division by zero)
- MP4: cap stts/stsz/stsc/stco tables at 50M entries; reject sample_rate=0 tracks; overflow check in sample offset accumulation; 64MB per-sample read cap
- MKV: cap string reads at 64KB; cap tracks at 128
- OGG: cap concurrent streams at 64

#### Audio — memory bounds and safety
- PipeWire ring buffer: modulo before write, bounds check after (prevented out-of-bounds unsafe write)
- Resampler: 1GB output cap on both linear and sinc paths; overflow guard on dst_frames × channels
- Decoder: 512MB cap on decode_all() accumulation (prevented unbounded memory growth)
- PCM encoder: checked_mul on num_samples × channels (prevented overflow)
- FLAC encoder: pre-allocate and reuse channel_samples Vec instead of per-frame allocation

#### Video — FFI safety and bounds checking
- VPX decoder: stride validation (stride >= width); checked_mul on row × stride; error on zero dimensions
- VPX encoder: bounds check src_start + width <= data.len() before unsafe copy; checked_mul on row offsets
- openh264 decoder: return error on invalid stride instead of silent empty frame
- rav1e encoder: checked_mul on (height-1) × stride for all planes
- Decoder framework: cap pending_frames queue at 64

#### AI — network and input safety
- Daimon: improved URL validation (require http:// or https://); response body 10MB size limit
- Transcription: 100MB WAV upload size limit
- Scene detector: validate histogram_bins > 0, default to 64

#### MCP server — protocol hardening
- Removed double File::open in probe (halved filesystem I/O per request)
- tarang_formats: read only first 32 bytes instead of entire file
- Max JSON-RPC message size: 10MB
- top_k parameter capped at 100
- Serialization errors now return proper error responses instead of empty strings

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

### Test coverage: 413 tests (was 391)
- FLAC roundtrip validation: encode → symphonia decode → sample comparison (mono, stereo, silence, CRC checks)
- FLAC edge cases: 24-bit encoding, single-sample blocks, max-amplitude, DC offset, multi-block, compression ratio
- MP4 muxer regression: roundtrip via demuxer, empty track, single sample, seek-back patching, stco offset verification
- Resampler accuracy: identity passthrough, downsample/upsample roundtrip, extreme ratios (24x), single sample, sinc vs linear SNR, energy conservation

### Versioning
- Switched from calendar versioning (YYYY.M.D) to semantic versioning for crates.io compatibility
- ADR 002 updated to document semver rationale

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
