# Tarang Roadmap

## Phase 1 — Foundation (Current)
- [x] Core types: codecs, formats, buffers, pipeline primitives
- [x] WAV demuxer (pure Rust)
- [x] Magic bytes format detection
- [x] Audio probe via symphonia
- [x] Video decoder framework (backend stubs)
- [x] AI content classification
- [x] CLI + MCP server (5 tools)
- [x] CI/CD pipelines

## Phase 2 — Container Parsing
- [ ] MP4/M4A demuxer (pure Rust — parse moov/mdat atoms)
- [ ] Matroska/WebM demuxer (pure Rust — EBML parser)
- [ ] OGG demuxer (pure Rust — page/packet extraction)

## Phase 3 — Video Codec FFI
- [ ] dav1d bindings (AV1 decoding)
- [ ] openh264 bindings (H.264 decoding)
- [ ] libvpx bindings (VP8/VP9 decoding)
- [ ] Safe wrapper types with lifetime management
- [ ] Hardware acceleration detection (VA-API, VDPAU)

## Phase 4 — Audio Pipeline
- [ ] Full symphonia decode pipeline (not just probe)
- [ ] Resampling (pure Rust)
- [ ] Channel mixing (stereo↔mono, 5.1 downmix)
- [ ] PipeWire output integration

## Phase 5 — AI Features
- [ ] Transcription routing to hoosh (Whisper models)
- [ ] Audio fingerprinting
- [ ] Scene detection in video
- [ ] Thumbnail generation at keyframes

## Phase 6 — Integration
- [ ] mpv backend (replace ffmpeg in mpv with tarang)
- [ ] AGNOS marketplace recipe
- [ ] MCP tools registered in daimon
- [ ] agnoshi intents ("play music", "probe file", "transcribe audio")
