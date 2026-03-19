# Tarang Roadmap

> **Principle**: FOSS codecs first, proprietary codecs next, wide coverage always in scope.

## Waiting on Upstream
- [ ] **VA-API encode pipeline completion** — surface upload, parameter buffers, bitstream readback. Blocked on `cros-codecs` releasing a version compatible with `cros-libva` 0.0.13 (current cros-codecs 0.0.6 depends on cros-libva 0.0.12). *(added 2026-03-16)*
- [ ] **rav1e `paste` dependency** — rav1e 0.8.1 depends on `paste` 1.0.15 which is unmaintained (RUSTSEC-2024-0436). No security vulnerability, but watch for rav1e release that drops or replaces it. *(added 2026-03-16)*

## Engineering Backlog *(added 2026-03-18)*

### Test Coverage
- [x] **tarang-audio**: Opus encoder tests (11 tests — error paths, sample rates, partial frames) *(done 2026-03-19)*
- [x] **tarang-audio**: AAC encoder tests (7 tests — buffer edges, flush, channel counts) *(done 2026-03-19)*
- [x] **tarang-audio**: PipeWire output tests (7 tests — ring buffer wraparound, full/empty, data integrity) *(done 2026-03-19)*
- [x] **tarang-video**: dav1d decoder tests (7 tests — creation, flush, timestamps) *(done 2026-03-19)*
- [x] **tarang-video**: rav1e encoder tests (9 tests — encode/flush, validation, dimension checks) *(done 2026-03-19)*
- [x] **tarang-ai**: daimon.rs unit tests (12 new tests — embeddings, RAG formatting, prompt building, parse edge cases) *(done 2026-03-19)*
- [x] **tarang-demux**: Integration tests (4 tests — WAV probe/read/seek, OGG Opus+Vorbis roundtrip, format detection) *(done 2026-03-19)*
- [x] **tarang-demux**: Error case tests (5 tests — truncated MP4/OGG/WAV, missing moov, zero channels) *(done 2026-03-19)*
- [x] **tarang (CLI)**: MCP protocol validation tests (4 tests — error/success response, open_and_probe) *(done 2026-03-19)*

### Performance
- [x] **tarang-audio/resample**: Pre-compute sinc lookup table for windowed sinc resampler *(done 2026-03-19)*
- [ ] **tarang-audio/resample**: SIMD vectorization for linear interpolation
- [x] **tarang-ai/scene**: Integer math for RGB→luminance — `(77*r+150*g+29*b)>>8` *(done 2026-03-19)*
- [x] **tarang-ai/thumbnail**: Integer YUV→RGB with pre-computed chroma contributions *(done 2026-03-19)*
- [ ] **tarang-demux/mux**: Mp4Muxer streaming write (currently accumulates all samples in memory — OOMs on large files)
- [ ] **tarang-audio/encode_flac**: LPC compression (currently verbatim-only — valid FLAC but uncompressed)

### Code Quality
- [x] **tarang-demux**: Extract shared EBML helpers (ebml.rs — 26 tests) *(done 2026-03-19)*
- [x] **tarang-ai**: Extract shared luminance computation (video_utils.rs — 7 tests) *(done 2026-03-19)*
- [x] **tarang-ai**: Extract shared sample format conversion (audio_utils.rs — 5 tests) *(done 2026-03-19)*
- [x] **tarang-audio/encode_aac**: Use `sample::I16_SCALE` constant *(done 2026-03-19)*
- [x] **tarang-audio/encode_aac**: Return errors from `flush()` *(done 2026-03-19)*
- [x] **tarang-demux**: Propagate stream position errors in mp4.rs *(done 2026-03-19)*
- [x] **tarang-video**: Remove dead `update_dims()` method in lib.rs *(done 2026-03-19)*
- [x] **tarang-video**: Standardize timestamp units — documented nanos convention *(done 2026-03-19)*

