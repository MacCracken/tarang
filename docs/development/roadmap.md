# Tarang Roadmap

> **Principle**: FOSS codecs first, proprietary codecs next, wide coverage always in scope.

## Waiting on Upstream
- [ ] **VA-API encode pipeline completion** — surface upload, parameter buffers, bitstream readback. Blocked on `cros-codecs` releasing a version compatible with `cros-libva` 0.0.13 (current cros-codecs 0.0.6 depends on cros-libva 0.0.12). *(added 2026-03-16)*
- [ ] **rav1e `paste` dependency** — rav1e 0.8.1 depends on `paste` 1.0.15 which is unmaintained (RUSTSEC-2024-0436). No security vulnerability, but watch for rav1e release that drops or replaces it. *(added 2026-03-16)*

## Engineering Backlog

### Testing
- [ ] Code review and audit pass — review all changes from 2026-03-19 refactor session (main.rs modularization, safety fixes, shared utilities, performance optimizations, FLAC LPC, Mp4Muxer streaming, SIMD resampler) for correctness, edge cases, and regressions
- [ ] FLAC encoder validation — decode FLAC output with symphonia or an external decoder to verify bitstream correctness for fixed prediction orders 1-4, Rice coding, and CRC checksums
- [ ] Expand FLAC test coverage — test 24-bit encoding, multi-block files, edge cases (single-sample blocks, max-amplitude signals, DC offset), verify compression ratio on real audio
- [ ] Mp4Muxer streaming write regression tests — verify large file handling (>4GB mdat), seek-back patching correctness, and roundtrip with Mp4Demuxer after the streaming refactor
- [ ] Resampler accuracy tests — compare SIMD-optimized linear resampler output against reference (pre-optimization) output to verify bit-exact equivalence

### Features
- [x] **tarang-audio/encode_flac**: Linear LPC prediction (Levinson-Durbin) — autocorrelation, Levinson-Durbin coefficient computation, quantization, LPC orders 1-8 tried alongside fixed 0-4, best method selected per channel *(done 2026-03-19)*
