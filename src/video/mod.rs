//! tarang-video — Video decoding and encoding for the Tarang media framework
//!
//! Provides thin Rust wrappers around native codec libraries:
//!
//! **Decoders**: dav1d (AV1), openh264 (H.264), libvpx (VP8/VP9)
//! **Encoders**: rav1e (AV1), openh264 (H.264), libvpx (VP8/VP9)
//!
//! Each codec is behind a feature flag. The Rust layer owns the pipeline,
//! memory management, and error handling. C codecs are called through safe
//! FFI boundaries.

#[cfg(feature = "dav1d")]
pub mod dav1d_dec;
#[cfg(feature = "openh264")]
pub mod openh264_dec;
#[cfg(feature = "openh264-enc")]
pub mod openh264_enc;
#[cfg(feature = "rav1e")]
pub mod rav1e_enc;
#[cfg(feature = "vaapi")]
pub mod vaapi_enc;
#[cfg(feature = "vaapi")]
pub mod vaapi_probe;
#[cfg(feature = "vpx")]
pub mod vpx_dec;
#[cfg(feature = "vpx-enc")]
pub mod vpx_enc;

#[cfg(feature = "dav1d")]
pub use dav1d_dec::Dav1dDecoder;
#[cfg(feature = "openh264")]
pub use openh264_dec::OpenH264Decoder;
#[cfg(feature = "openh264-enc")]
pub use openh264_enc::{OpenH264Encoder, OpenH264EncoderConfig};
#[cfg(feature = "rav1e")]
pub use rav1e_enc::{Rav1eConfig, Rav1eEncoder};
#[cfg(feature = "vaapi")]
pub use vaapi_enc::{VaapiEncoder, VaapiEncoderConfig};
#[cfg(feature = "vaapi")]
pub use vaapi_probe::{HwAccelReport, HwCodecCapability, HwCodecDirection, probe_vaapi};
#[cfg(feature = "vpx")]
pub use vpx_dec::VpxDecoder;
#[cfg(feature = "vpx-enc")]
pub use vpx_enc::{VpxEncoder, VpxEncoderConfig};

use crate::core::{PixelFormat, Result, TarangError, VideoCodec, VideoFrame, VideoStreamInfo};
use std::time::Duration;

/// Video decoder status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderStatus {
    /// Ready to accept input
    Ready,
    /// Needs more data before producing output
    NeedsInput,
    /// Has a decoded frame available
    HasOutput,
    /// Decoder has been flushed
    Flushed,
}

/// Codec backend implementation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderBackend {
    /// dav1d — AV1 decoder (C FFI)
    Dav1d,
    /// openh264 — H.264 decoder (C FFI, Cisco BSD)
    OpenH264,
    /// libvpx — VP8/VP9 decoder (C FFI)
    LibVpx,
    /// Software fallback (pure Rust, limited)
    Software,
    /// VA-API hardware acceleration
    Vaapi,
}

impl std::fmt::Display for DecoderBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dav1d => write!(f, "dav1d"),
            Self::OpenH264 => write!(f, "openh264"),
            Self::LibVpx => write!(f, "libvpx"),
            Self::Software => write!(f, "software"),
            Self::Vaapi => write!(f, "vaapi"),
        }
    }
}

/// Video decoder configuration
#[derive(Debug, Clone)]
pub struct DecoderConfig {
    pub codec: VideoCodec,
    pub backend: DecoderBackend,
    pub thread_count: u32,
    pub hw_accel: bool,
}

impl DecoderConfig {
    /// Create a default config for the given codec
    pub fn for_codec(codec: VideoCodec) -> Result<Self> {
        let backend = match codec {
            VideoCodec::Av1 => {
                if !cfg!(feature = "dav1d") {
                    return Err(TarangError::UnsupportedCodec(
                        "AV1 decoding requires the `dav1d` feature".to_string(),
                    ));
                }
                DecoderBackend::Dav1d
            }
            VideoCodec::H264 => {
                if !cfg!(feature = "openh264") {
                    return Err(TarangError::UnsupportedCodec(
                        "H.264 decoding requires the `openh264` feature".to_string(),
                    ));
                }
                DecoderBackend::OpenH264
            }
            VideoCodec::Vp8 | VideoCodec::Vp9 => {
                if !cfg!(feature = "vpx") {
                    return Err(TarangError::UnsupportedCodec(
                        "VP8/VP9 decoding requires the `vpx` feature".to_string(),
                    ));
                }
                DecoderBackend::LibVpx
            }
            VideoCodec::H265 => {
                return Err(TarangError::UnsupportedCodec(
                    "H.265 not yet supported — no BSD-licensed decoder available".to_string(),
                ));
            }
            VideoCodec::Theora => DecoderBackend::Software,
        };

        Ok(Self {
            codec,
            backend,
            thread_count: num_cpus(),
            hw_accel: false,
        })
    }
}

/// Internal backend handle — holds the actual FFI decoder.
enum BackendInner {
    #[cfg(feature = "dav1d")]
    Dav1d(Dav1dDecoder),
    #[cfg(feature = "openh264")]
    OpenH264(OpenH264Decoder),
    #[cfg(feature = "vpx")]
    Vpx(VpxDecoder),
    /// Software / stub — produces black frames (used for Theora until a Rust decoder exists)
    Stub,
}

/// Unified video decoder — dispatches to the appropriate FFI backend
/// based on the configured codec.
pub struct VideoDecoder {
    config: DecoderConfig,
    backend: BackendInner,
    status: DecoderStatus,
    frames_decoded: u64,
    width: u32,
    height: u32,
    /// Frames buffered from backends that return multiple frames per call
    pending_frames: Vec<VideoFrame>,
}

impl VideoDecoder {
    /// Create a new video decoder, initializing the appropriate FFI backend.
    pub fn new(config: DecoderConfig) -> Result<Self> {
        let backend = match config.backend {
            #[cfg(feature = "dav1d")]
            DecoderBackend::Dav1d => BackendInner::Dav1d(Dav1dDecoder::new()?),
            #[cfg(feature = "openh264")]
            DecoderBackend::OpenH264 => BackendInner::OpenH264(OpenH264Decoder::new()?),
            #[cfg(feature = "vpx")]
            DecoderBackend::LibVpx => BackendInner::Vpx(VpxDecoder::new(config.codec)?),
            _ => BackendInner::Stub,
        };

        Ok(Self {
            config,
            backend,
            status: DecoderStatus::Ready,
            frames_decoded: 0,
            width: 0,
            height: 0,
            pending_frames: Vec::new(),
        })
    }

    /// Initialize from stream info (sets expected dimensions).
    pub fn init(&mut self, info: &VideoStreamInfo) {
        self.width = info.width;
        self.height = info.height;
    }

    /// Get the codec
    pub fn codec(&self) -> VideoCodec {
        self.config.codec
    }

    /// Get the backend
    pub fn backend(&self) -> DecoderBackend {
        self.config.backend
    }

    /// Get current status
    pub fn status(&self) -> DecoderStatus {
        self.status
    }

    /// Get total frames decoded
    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }

    /// Get configured dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Feed compressed data to the decoder.
    pub fn send_packet(&mut self, data: &[u8], timestamp: Duration) -> Result<()> {
        if data.is_empty() {
            return Err(TarangError::DecodeError("empty packet".to_string()));
        }

        tracing::trace!(
            codec = %self.config.codec,
            data_len = data.len(),
            pending = self.pending_frames.len(),
            "video packet sent"
        );

        // Collect decoded frames into a local vec to avoid borrow conflicts
        // with self.backend and self.pending_frames/width/height
        let mut decoded = Vec::new();

        match &mut self.backend {
            #[cfg(feature = "dav1d")]
            BackendInner::Dav1d(dec) => {
                dec.send_data(data, timestamp.as_nanos() as i64)?;
                if let Some(frame) = dec.get_frame()? {
                    decoded.push(frame);
                }
            }
            #[cfg(feature = "openh264")]
            BackendInner::OpenH264(dec) => {
                if let Some(frame) = dec.decode(data, timestamp)? {
                    decoded.push(frame);
                }
            }
            #[cfg(feature = "vpx")]
            BackendInner::Vpx(dec) => {
                decoded = dec.decode(data, timestamp)?;
            }
            BackendInner::Stub => {
                let w = if self.width > 0 { self.width } else { 320 };
                let h = if self.height > 0 { self.height } else { 240 };
                let size = crate::core::yuv420p_frame_size(w, h);
                decoded.push(VideoFrame {
                    data: bytes::Bytes::from(vec![0u8; size]),
                    pixel_format: PixelFormat::Yuv420p,
                    width: w,
                    height: h,
                    timestamp,
                });
            }
        }

        for frame in decoded {
            if self.width == 0 {
                self.width = frame.width;
                self.height = frame.height;
            }
            self.pending_frames.push(frame);
        }

        self.status = if self.pending_frames.is_empty() {
            DecoderStatus::NeedsInput
        } else {
            DecoderStatus::HasOutput
        };

        Ok(())
    }

    /// Retrieve a decoded frame if available.
    pub fn receive_frame(&mut self) -> Result<VideoFrame> {
        if let Some(frame) = self.pending_frames.pop() {
            self.frames_decoded += 1;
            self.status = if self.pending_frames.is_empty() {
                DecoderStatus::NeedsInput
            } else {
                DecoderStatus::HasOutput
            };
            return Ok(frame);
        }

        // For dav1d, try pulling another frame (it can buffer internally)
        #[cfg(feature = "dav1d")]
        if let BackendInner::Dav1d(dec) = &mut self.backend
            && let Some(frame) = dec.get_frame()?
        {
            if self.width == 0 {
                self.width = frame.width;
                self.height = frame.height;
            }
            self.frames_decoded += 1;
            self.status = DecoderStatus::NeedsInput;
            return Ok(frame);
        }

        self.status = DecoderStatus::NeedsInput;
        Err(TarangError::DecodeError("no frame available".to_string()))
    }

    /// Flush the decoder (signal end of stream) and drain buffered frames.
    pub fn flush(&mut self) -> Result<()> {
        // Collect flushed frames into a local vec to avoid borrow conflicts
        #[allow(unused_mut)]
        let mut flushed: Vec<VideoFrame> = Vec::new();

        match &mut self.backend {
            #[cfg(feature = "openh264")]
            BackendInner::OpenH264(dec) => {
                flushed = dec.flush()?;
            }
            #[cfg(feature = "dav1d")]
            BackendInner::Dav1d(dec) => {
                while let Ok(Some(frame)) = dec.get_frame() {
                    flushed.push(frame);
                }
            }
            _ => {}
        }

        for frame in flushed {
            if self.width == 0 {
                self.width = frame.width;
                self.height = frame.height;
            }
            self.pending_frames.push(frame);
        }

        self.status = if self.pending_frames.is_empty() {
            DecoderStatus::Flushed
        } else {
            DecoderStatus::HasOutput
        };
        Ok(())
    }
}

/// List video codecs and their backends (only includes compiled-in backends)
pub fn supported_codecs() -> Vec<(VideoCodec, DecoderBackend)> {
    let mut codecs = Vec::new();
    if cfg!(feature = "dav1d") {
        codecs.push((VideoCodec::Av1, DecoderBackend::Dav1d));
    }
    if cfg!(feature = "openh264") {
        codecs.push((VideoCodec::H264, DecoderBackend::OpenH264));
    }
    if cfg!(feature = "vpx") {
        codecs.push((VideoCodec::Vp8, DecoderBackend::LibVpx));
        codecs.push((VideoCodec::Vp9, DecoderBackend::LibVpx));
    }
    codecs.push((VideoCodec::Theora, DecoderBackend::Software));
    codecs
}

fn num_cpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_for_av1() {
        let result = DecoderConfig::for_codec(VideoCodec::Av1);
        if cfg!(feature = "dav1d") {
            let config = result.unwrap();
            assert_eq!(config.backend, DecoderBackend::Dav1d);
            assert_eq!(config.codec, VideoCodec::Av1);
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    fn config_for_h264() {
        let result = DecoderConfig::for_codec(VideoCodec::H264);
        if cfg!(feature = "openh264") {
            assert_eq!(result.unwrap().backend, DecoderBackend::OpenH264);
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    fn config_for_vp9() {
        let result = DecoderConfig::for_codec(VideoCodec::Vp9);
        if cfg!(feature = "vpx") {
            assert_eq!(result.unwrap().backend, DecoderBackend::LibVpx);
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    fn config_for_vp8() {
        let result = DecoderConfig::for_codec(VideoCodec::Vp8);
        if cfg!(feature = "vpx") {
            assert_eq!(result.unwrap().backend, DecoderBackend::LibVpx);
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    fn h265_unsupported() {
        assert!(DecoderConfig::for_codec(VideoCodec::H265).is_err());
    }

    #[test]
    fn theora_software() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        assert_eq!(config.backend, DecoderBackend::Software);
    }

    #[test]
    fn decoder_creation() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let decoder = VideoDecoder::new(config).unwrap();
        assert_eq!(decoder.codec(), VideoCodec::Theora);
        assert_eq!(decoder.backend(), DecoderBackend::Software);
        assert_eq!(decoder.status(), DecoderStatus::Ready);
        assert_eq!(decoder.frames_decoded(), 0);
    }

    #[test]
    fn decoder_init() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        decoder.init(&VideoStreamInfo {
            codec: VideoCodec::Theora,
            width: 1920,
            height: 1080,
            pixel_format: PixelFormat::Yuv420p,
            frame_rate: 30.0,
            bitrate: Some(5_000_000),
            duration: None,
        });
        assert_eq!(decoder.dimensions(), (1920, 1080));
    }

    #[test]
    fn decode_cycle() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        decoder.init(&VideoStreamInfo {
            codec: VideoCodec::Theora,
            width: 640,
            height: 480,
            pixel_format: PixelFormat::Yuv420p,
            frame_rate: 24.0,
            bitrate: None,
            duration: None,
        });

        // Can't receive before sending
        assert!(decoder.receive_frame().is_err());

        // Send a packet
        decoder.send_packet(&[0, 1, 2, 3], Duration::ZERO).unwrap();
        assert_eq!(decoder.status(), DecoderStatus::HasOutput);

        // Receive the frame
        let frame = decoder.receive_frame().unwrap();
        assert_eq!(frame.width, 640);
        assert_eq!(frame.height, 480);
        assert_eq!(frame.pixel_format, PixelFormat::Yuv420p);
        assert_eq!(decoder.frames_decoded(), 1);
        assert_eq!(decoder.status(), DecoderStatus::NeedsInput);
    }

    #[test]
    fn empty_packet_error() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        assert!(decoder.send_packet(&[], Duration::ZERO).is_err());
    }

    #[test]
    fn flush() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        decoder.flush().unwrap();
        assert_eq!(decoder.status(), DecoderStatus::Flushed);
    }

    #[test]
    fn supported_codecs_list() {
        let codecs = supported_codecs();
        assert!(codecs.contains(&(VideoCodec::Theora, DecoderBackend::Software)));
        assert!(!codecs.iter().any(|(c, _)| *c == VideoCodec::H265));
    }

    #[test]
    fn backend_display() {
        assert_eq!(DecoderBackend::Dav1d.to_string(), "dav1d");
        assert_eq!(DecoderBackend::OpenH264.to_string(), "openh264");
        assert_eq!(DecoderBackend::LibVpx.to_string(), "libvpx");
        assert_eq!(DecoderBackend::Software.to_string(), "software");
        assert_eq!(DecoderBackend::Vaapi.to_string(), "vaapi");
    }

    #[test]
    fn multiple_frames() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();

        for i in 0..5 {
            decoder
                .send_packet(&[i as u8; 100], Duration::from_millis(i * 33))
                .unwrap();
            let _frame = decoder.receive_frame().unwrap();
        }
        assert_eq!(decoder.frames_decoded(), 5);
    }

    #[test]
    fn decoder_status_transitions() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        assert_eq!(decoder.status(), DecoderStatus::Ready);

        decoder.send_packet(&[1, 2, 3], Duration::ZERO).unwrap();
        assert_eq!(decoder.status(), DecoderStatus::HasOutput);

        let _ = decoder.receive_frame().unwrap();
        assert_eq!(decoder.status(), DecoderStatus::NeedsInput);

        decoder.flush().unwrap();
        assert_eq!(decoder.status(), DecoderStatus::Flushed);
    }

    #[test]
    fn receive_without_send_errors() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        assert!(decoder.receive_frame().is_err());
    }

    #[test]
    fn decoder_default_dimensions() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        assert_eq!(decoder.dimensions(), (0, 0));

        // Without init, stub uses default 320x240
        decoder.send_packet(&[0xFF], Duration::ZERO).unwrap();
        let frame = decoder.receive_frame().unwrap();
        assert_eq!(frame.width, 320);
        assert_eq!(frame.height, 240);
        assert_eq!(frame.pixel_format, PixelFormat::Yuv420p);
    }

    #[test]
    fn decoder_frame_timestamps() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();

        // Stub backend passes through the timestamp from send_packet
        decoder
            .send_packet(&[1], Duration::from_millis(100))
            .unwrap();
        let frame = decoder.receive_frame().unwrap();
        assert_eq!(frame.timestamp, Duration::from_millis(100));

        decoder
            .send_packet(&[2], Duration::from_millis(200))
            .unwrap();
        let frame = decoder.receive_frame().unwrap();
        assert_eq!(frame.timestamp, Duration::from_millis(200));
    }

    #[test]
    fn decoder_config_thread_count() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        assert!(config.thread_count >= 1);
        assert!(!config.hw_accel);
    }

    #[test]
    fn decoder_status_equality() {
        assert_eq!(DecoderStatus::Ready, DecoderStatus::Ready);
        assert_ne!(DecoderStatus::Ready, DecoderStatus::Flushed);
        assert_ne!(DecoderStatus::HasOutput, DecoderStatus::NeedsInput);
    }

    #[test]
    fn supported_codecs_no_duplicates() {
        let codecs = supported_codecs();
        for (i, a) in codecs.iter().enumerate() {
            for b in &codecs[i + 1..] {
                assert_ne!(a, b, "duplicate codec entry");
            }
        }
    }

    #[test]
    fn stub_frame_is_yuv420p_sized() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        decoder.init(&VideoStreamInfo {
            codec: VideoCodec::Theora,
            width: 64,
            height: 48,
            pixel_format: PixelFormat::Yuv420p,
            frame_rate: 30.0,
            bitrate: None,
            duration: None,
        });

        decoder.send_packet(&[0x42], Duration::ZERO).unwrap();
        let frame = decoder.receive_frame().unwrap();
        let expected = crate::core::yuv420p_frame_size(64, 48);
        assert_eq!(frame.data.len(), expected);
    }

    #[test]
    fn flush_on_empty_is_flushed() {
        let config = DecoderConfig::for_codec(VideoCodec::Theora).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        decoder.flush().unwrap();
        assert_eq!(decoder.status(), DecoderStatus::Flushed);
    }
}
