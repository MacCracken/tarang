# Tarang Roadmap

> **v2.0.0** — Cyrius port. 13 source modules, 311 assertions, 189KB binary, 365ms compile.
> Pure Cyrius media framework. Shravan v2.1.1 as codec engine (via deps).

Completed items are in [CHANGELOG.md](../../CHANGELOG.md).

---

## Cyrius Port Status

### Completed (v2.0.0)

- [x] **core** — enums, structs, error codes, magic detection, byte helpers
- [x] **demux/wav** — RIFF WAVE parser, probe/read/seek
- [x] **demux/ogg** — OGG page parser, Vorbis/Opus/FLAC codec detection, duration scan, bisection seek
- [x] **demux/mp4** — ISOBMFF box tree (ftyp/moov/trak/mdia/stbl), sample tables (stts/stsc/stsz/stco/co64), seek
- [x] **demux/mkv** — EBML parser, Matroska/WebM, audio+video track detection, cluster seek
- [x] **mux/wav** — WAV muxer with header patching
- [x] **mux/ogg** — OGG muxer (Opus/Vorbis headers + page wrapping)
- [x] **audio** — resample (linear), channel mixing (stereo/mono), loudness (LUFS), gain
- [x] **ai/fingerprint** — inline radix-2 FFT, chroma binning, differential hashing, matching
- [x] **ai/scene** — luminance histogram, chi-squared distance, hard cut detection
- [x] **ai/daimon** — config, client stub (HTTP integration pending net.cyr)
- [x] **ai/classify** — content type classification (music/speech/movie), quality scoring, tagging
- [x] **mcp** — JSON-RPC helpers, tool handlers (probe/analyze/codecs), stdio server
- [x] **cli** — probe/codecs/version/help subcommands via args.cyr

### Blocked

| Module | Blocked on | Notes |
|--------|-----------|-------|
| **ai/thumbnail** | Image encode lib | Need JPEG/PNG encoder in Cyrius (or new project) |
| **ai/transcribe** | hoosh conversion | hoosh not yet ported to Cyrius |
| **ai/diarize** | hoosh conversion | Speaker diarization routes through hoosh |
| **ai/daimon HTTP** | net.cyr integration | Daimon client stub ready, needs actual HTTP calls |
| **video/ codecs** | drishti-* projects | Pure Cyrius video decoders (see Cyrius roadmap) |
| **hwaccel** | ai-hwaccel conversion | Blocked on libro wrap completion |
| **mcp full** | bote conversion | Subprocess bridge in place, replace when bote converts |
| **audio/effects** | Priority | DSP chain (EQ, compressor, reverb) — not blocked, just lower priority |
| **audio/output** | Platform | ALSA/PipeWire output — needs platform-specific syscalls |

---

## Post-v2 Roadmap

### Pure Cyrius codec backends

Tracked on [Cyrius roadmap](https://github.com/MacCracken/cyrius) as drishti-* projects.
Each is an independent repo following the shravan model.

| Codec | Project | Replaces | Status |
|-------|---------|----------|--------|
| AV1 decode | drishti-av1 | dav1d | Not started |
| AV1 encode | drishti-rav1e | rav1e | Not started |
| H.264 decode/encode | drishti-h264 | openh264 | Not started |
| H.265 decode | drishti-h265 | libde265 | Not started |
| VP8/VP9 decode/encode | drishti-vpx | libvpx | Not started |

### Platform support

- [ ] **macOS VideoToolbox** — hardware H.264/H.265 (needs Mach-O backend in Cyrius)
- [ ] **Windows Media Foundation** — hardware H.264/H.265 (needs PE backend in Cyrius)
- [ ] **ALSA audio output** — direct syscall-based Linux audio playback
- [ ] **PipeWire output** — socket protocol for modern Linux audio

### Advanced features

- [ ] **Muxer streaming** — MP4/MKV muxers (currently only WAV + OGG)
- [ ] **Fragmented MP4** — fMP4 demux support (moof/traf/trun parsing)
- [ ] **Parallel decode** — multi-threaded via thread.cyr
- [ ] **GPU fingerprinting** — Vulkan compute for FFT (needs mabda conversion)
- [ ] **Real-time pipeline** — lock-free audio graph
- [ ] **Offline transcription** — when hoosh converts, direct inference
- [ ] **SIMD audio** — explicit SIMD for resample/mix/fingerprint via simd.cyr

### Integration milestones

| Milestone | Depends on | Unlocks |
|-----------|-----------|---------|
| **shravan full integration** | shravan lib.cyr split | Direct codec_open/encode from tarang |
| **bote conversion** | bote Cyrius port | Native MCP server (remove subprocess bridge) |
| **hoosh conversion** | hoosh Cyrius port | Transcription + diarization |
| **ai-hwaccel conversion** | libro wrap | Hardware detection module |
| **image.cyr** | New project or FFI | Thumbnail generation (JPEG/PNG) |

---

## Non-goals

- **Full ffmpeg replacement** — tarang covers decode, encode, demux, mux, and analysis. No filter graph, network protocols, or device capture.
- **Proprietary codecs** — no Dolby, DTS. Only FOSS and permissive-licensed.
- **GUI** — tarang is a library and CLI. Jalwa handles GUI playback.
- **Streaming server** — tarang produces packets. HTTP/WebRTC serving is the consumer's job.
