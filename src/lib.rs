//! # tarang — AI-native Rust media framework
//!
//! Tarang provides container parsing, audio/video decoding, media processing,
//! and AI-powered content analysis as a single Rust crate. It replaces ffmpeg
//! with a Rust-owned pipeline: pure Rust audio decoding via symphonia, pure Rust
//! container demuxing/muxing, thin C FFI wrappers for video codecs, and LLM
//! integration through the AGNOS agent ecosystem.
//!
//! ## Supported formats
//!
//! ### Audio codecs
//!
//! | Codec | Decode | Encode | Backend |
//! |-------|--------|--------|---------|
//! | MP3 | yes | - | symphonia (pure Rust) |
//! | FLAC | yes | yes | symphonia / pure Rust encoder |
//! | Vorbis | yes | - | symphonia |
//! | Opus | yes | yes | symphonia / libopus FFI (`opus-enc`) |
//! | AAC | yes | yes* | symphonia / fdk-aac FFI (`aac-enc`) |
//! | ALAC | yes | - | symphonia |
//! | PCM | yes | yes | pure Rust (16/24/32-bit) |
//!
//! ### Video codecs (feature-gated)
//!
//! | Codec | Decode | Encode | Feature |
//! |-------|--------|--------|---------|
//! | AV1 | dav1d | rav1e | `dav1d` / `rav1e` |
//! | H.264 | openh264 | openh264 | `openh264` / `openh264-enc` |
//! | VP8/VP9 | libvpx | libvpx | `vpx` / `vpx-enc` |
//! | H.265 | VA-API hw only | VA-API | `vaapi` + `hwaccel` |
//!
//! *\* AAC encoding requires the `libfdk-aac` system library (LGPL-2.1).
//! No pure-Rust AAC encoder exists yet. If linking fdk-aac is not possible,
//! use Opus (`opus-enc`) or FLAC as alternatives.*
//!
//! *H.265 decode has no free software decoder. Use
//! `DecoderConfig::for_codec_auto()` (with the `hwaccel` feature)
//! to decode via VA-API hardware acceleration on supported GPUs.*
//!
//! ### Containers (pure Rust)
//!
//! MP4/M4A, MKV/WebM (EBML), OGG (Vorbis/Opus/FLAC), WAV, FLAC, MP3, AVI
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use std::io::Cursor;
//! use tarang::demux::{Demuxer, WavDemuxer};
//!
//! # fn main() -> tarang::core::Result<()> {
//! // Open and probe a WAV file
//! let file = std::fs::File::open("audio.wav")?;
//! let mut demuxer = WavDemuxer::new(file);
//! let info = demuxer.probe()?;
//! println!("Format: {:?}, streams: {}", info.format, info.streams.len());
//!
//! // Read packets
//! while let Ok(packet) = demuxer.next_packet() {
//!     println!("Packet: {} bytes, ts={:?}", packet.data.len(), packet.timestamp);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Decode and process audio
//!
//! ```rust,no_run
//! # fn main() -> tarang::core::Result<()> {
//! // Decode an audio file
//! let file = std::fs::File::open("song.mp3")?;
//! let mut decoder = tarang::audio::FileDecoder::open(
//!     Box::new(file), Some("mp3")
//! )?;
//! let audio = decoder.decode_all()?;
//!
//! // Resample 44.1kHz → 16kHz
//! let resampled = tarang::audio::resample(&audio, 16000)?;
//!
//! // Mix stereo → mono
//! let mono = tarang::audio::mix_channels(
//!     &resampled, tarang::audio::ChannelLayout::Mono
//! )?;
//! # Ok(())
//! # }
//! ```
//!
//! ## AI analysis
//!
//! ```rust,no_run
//! use tarang::ai;
//!
//! # fn example(audio: &tarang::core::AudioBuffer) {
//! // Audio fingerprinting
//! let fp = ai::compute_fingerprint(audio, &Default::default());
//!
//! // Scene detection (feed video frames)
//! let mut detector = ai::SceneDetector::new(Default::default());
//! // detector.feed_frame(&frame);
//! let boundaries = detector.finish();
//! # }
//! ```
//!
//! ## Feature flags
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `full` | Enable all codec backends (Linux) |
//! | `portable` | All codecs except platform-specific (vaapi, pipewire excluded) |
//! | `dav1d` | AV1 decoding via libdav1d |
//! | `vpx` | VP8/VP9 decoding via libvpx |
//! | `vpx-enc` | VP8/VP9 encoding via libvpx |
//! | `openh264` | H.264 decoding via openh264 |
//! | `openh264-enc` | H.264 encoding via openh264 |
//! | `rav1e` | AV1 encoding via rav1e (pure Rust) |
//! | `vaapi` | VA-API hardware acceleration probe/encode |
//! | `h265-decode` | H.265 software decode via libde265 (LGPL) |
//! | `hwaccel` | Hardware accelerator detection via ai-hwaccel |
//! | `cpal-output` | Cross-platform audio output (CoreAudio, WASAPI, ALSA) |
//! | `pipewire` | PipeWire audio output (Linux) |
//! | `opus-enc` | Opus encoding via libopus |
//! | `aac-enc` | AAC encoding via fdk-aac |
//! | `aac-dec` | AAC decoding via fdk-aac |
//!
//! Default features: none. Audio decoding (symphonia) and all container
//! demuxing/muxing are always available.
//!
//! ## CLI
//!
//! ```bash
//! tarang probe song.flac        # media info
//! tarang analyze movie.mp4      # AI classification
//! tarang codecs                 # list supported codecs
//! tarang mcp                    # MCP server (JSON-RPC over stdio)
//! ```
//!
//! ## Minimum Supported Rust Version
//!
//! Rust 1.89 (edition 2024).
//!
//! ## Versioning
//!
//! Semantic versioning (`MAJOR.MINOR.PATCH`). Pre-1.0: minor bumps may
//! include breaking changes. Version managed via `scripts/version-bump.sh`.

/// Core types: codecs, formats, buffers, errors, media metadata.
pub mod core;

/// Audio decoding, encoding, resampling, mixing, and output.
pub mod audio;

/// Container demuxing and muxing (MP4, MKV/WebM, OGG, WAV).
pub mod demux;

/// Video decoding and encoding via FFI (dav1d, openh264, libvpx, rav1e, VA-API).
pub mod video;

/// AI-powered media analysis: fingerprinting, scene detection, thumbnails, transcription.
pub mod ai;

/// Hardware accelerator detection via ai-hwaccel (GPU, NPU, TPU).
#[cfg(feature = "hwaccel")]
pub mod hwaccel;
