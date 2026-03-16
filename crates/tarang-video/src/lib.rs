//! tarang-video — Video decoding for the Tarang media framework
//!
//! Provides thin Rust wrappers around native C codec libraries:
//! - dav1d for AV1
//! - openh264 for H.264
//! - libvpx for VP8/VP9
//!
//! The Rust layer owns the pipeline, memory management, and error handling.
//! C codecs are called through safe FFI boundaries.

#[cfg(feature = "dav1d")]
pub mod dav1d_dec;
#[cfg(feature = "vpx")]
pub mod vpx_dec;
#[cfg(feature = "rav1e")]
pub mod rav1e_enc;

#[cfg(feature = "dav1d")]
pub use dav1d_dec::Dav1dDecoder;
#[cfg(feature = "vpx")]
pub use vpx_dec::VpxDecoder;
#[cfg(feature = "rav1e")]
pub use rav1e_enc::{Rav1eConfig, Rav1eEncoder};

use std::time::Duration;
use tarang_core::{PixelFormat, Result, TarangError, VideoCodec, VideoFrame, VideoStreamInfo};

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
}

impl std::fmt::Display for DecoderBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dav1d => write!(f, "dav1d"),
            Self::OpenH264 => write!(f, "openh264"),
            Self::LibVpx => write!(f, "libvpx"),
            Self::Software => write!(f, "software"),
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
            VideoCodec::Av1 => DecoderBackend::Dav1d,
            VideoCodec::H264 => DecoderBackend::OpenH264,
            VideoCodec::Vp8 | VideoCodec::Vp9 => DecoderBackend::LibVpx,
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

/// Video decoder instance
pub struct VideoDecoder {
    config: DecoderConfig,
    status: DecoderStatus,
    frames_decoded: u64,
    width: u32,
    height: u32,
}

impl VideoDecoder {
    /// Create a new video decoder
    pub fn new(config: DecoderConfig) -> Result<Self> {
        Ok(Self {
            config,
            status: DecoderStatus::Ready,
            frames_decoded: 0,
            width: 0,
            height: 0,
        })
    }

    /// Initialize from stream info
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

    /// Feed compressed data to the decoder
    pub fn send_packet(&mut self, data: &[u8], _timestamp: Duration) -> Result<()> {
        if data.is_empty() {
            return Err(TarangError::DecodeError("empty packet".to_string()));
        }

        // TODO: FFI call to actual codec backend
        // For now, transition state
        self.status = DecoderStatus::HasOutput;
        Ok(())
    }

    /// Retrieve a decoded frame if available
    pub fn receive_frame(&mut self) -> Result<VideoFrame> {
        if self.status != DecoderStatus::HasOutput {
            return Err(TarangError::DecodeError("no frame available".to_string()));
        }

        // TODO: FFI call to retrieve decoded frame from backend
        // Placeholder: produce a black frame at configured dimensions
        let w = if self.width > 0 { self.width } else { 320 };
        let h = if self.height > 0 { self.height } else { 240 };
        let data_size = (w * h * 3) as usize; // RGB24

        self.frames_decoded += 1;
        self.status = DecoderStatus::NeedsInput;

        Ok(VideoFrame {
            data: bytes::Bytes::from(vec![0u8; data_size]),
            pixel_format: PixelFormat::Rgb24,
            width: w,
            height: h,
            timestamp: Duration::from_secs_f64(self.frames_decoded as f64 / 30.0),
        })
    }

    /// Flush the decoder (signal end of stream)
    pub fn flush(&mut self) -> Result<()> {
        self.status = DecoderStatus::Flushed;
        Ok(())
    }
}

/// List video codecs and their backends
pub fn supported_codecs() -> Vec<(VideoCodec, DecoderBackend)> {
    vec![
        (VideoCodec::Av1, DecoderBackend::Dav1d),
        (VideoCodec::H264, DecoderBackend::OpenH264),
        (VideoCodec::Vp8, DecoderBackend::LibVpx),
        (VideoCodec::Vp9, DecoderBackend::LibVpx),
        (VideoCodec::Theora, DecoderBackend::Software),
    ]
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
        let config = DecoderConfig::for_codec(VideoCodec::Av1).unwrap();
        assert_eq!(config.backend, DecoderBackend::Dav1d);
        assert_eq!(config.codec, VideoCodec::Av1);
    }

    #[test]
    fn config_for_h264() {
        let config = DecoderConfig::for_codec(VideoCodec::H264).unwrap();
        assert_eq!(config.backend, DecoderBackend::OpenH264);
    }

    #[test]
    fn config_for_vp9() {
        let config = DecoderConfig::for_codec(VideoCodec::Vp9).unwrap();
        assert_eq!(config.backend, DecoderBackend::LibVpx);
    }

    #[test]
    fn config_for_vp8() {
        let config = DecoderConfig::for_codec(VideoCodec::Vp8).unwrap();
        assert_eq!(config.backend, DecoderBackend::LibVpx);
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
        let config = DecoderConfig::for_codec(VideoCodec::Av1).unwrap();
        let decoder = VideoDecoder::new(config).unwrap();
        assert_eq!(decoder.codec(), VideoCodec::Av1);
        assert_eq!(decoder.backend(), DecoderBackend::Dav1d);
        assert_eq!(decoder.status(), DecoderStatus::Ready);
        assert_eq!(decoder.frames_decoded(), 0);
    }

    #[test]
    fn decoder_init() {
        let config = DecoderConfig::for_codec(VideoCodec::H264).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        decoder.init(&VideoStreamInfo {
            codec: VideoCodec::H264,
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
        let config = DecoderConfig::for_codec(VideoCodec::Av1).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        decoder.init(&VideoStreamInfo {
            codec: VideoCodec::Av1,
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
        assert_eq!(decoder.frames_decoded(), 1);
        assert_eq!(decoder.status(), DecoderStatus::NeedsInput);
    }

    #[test]
    fn empty_packet_error() {
        let config = DecoderConfig::for_codec(VideoCodec::H264).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        assert!(decoder.send_packet(&[], Duration::ZERO).is_err());
    }

    #[test]
    fn flush() {
        let config = DecoderConfig::for_codec(VideoCodec::Vp9).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();
        decoder.flush().unwrap();
        assert_eq!(decoder.status(), DecoderStatus::Flushed);
    }

    #[test]
    fn supported_codecs_list() {
        let codecs = supported_codecs();
        assert!(codecs.contains(&(VideoCodec::Av1, DecoderBackend::Dav1d)));
        assert!(codecs.contains(&(VideoCodec::H264, DecoderBackend::OpenH264)));
        assert!(codecs.contains(&(VideoCodec::Vp9, DecoderBackend::LibVpx)));
        assert!(!codecs.iter().any(|(c, _)| *c == VideoCodec::H265));
    }

    #[test]
    fn backend_display() {
        assert_eq!(DecoderBackend::Dav1d.to_string(), "dav1d");
        assert_eq!(DecoderBackend::OpenH264.to_string(), "openh264");
        assert_eq!(DecoderBackend::LibVpx.to_string(), "libvpx");
        assert_eq!(DecoderBackend::Software.to_string(), "software");
    }

    #[test]
    fn multiple_frames() {
        let config = DecoderConfig::for_codec(VideoCodec::H264).unwrap();
        let mut decoder = VideoDecoder::new(config).unwrap();

        for i in 0..5 {
            decoder
                .send_packet(&[i as u8; 100], Duration::from_millis(i * 33))
                .unwrap();
            let _frame = decoder.receive_frame().unwrap();
        }
        assert_eq!(decoder.frames_decoded(), 5);
    }
}
