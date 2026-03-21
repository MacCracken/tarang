//! H.264 encoding via openh264 (Cisco BSD-2-Clause)
//!
//! Safe Rust wrapper around openh264 for H.264 encoding.
//! Requires the `openh264-enc` feature.
//!
//! # Example
//!
//! ```rust,ignore
//! use tarang::video::openh264_enc::{OpenH264Encoder, OpenH264EncoderConfig};
//!
//! let config = OpenH264EncoderConfig { width: 1280, height: 720, ..Default::default() };
//! let mut encoder = OpenH264Encoder::new(&config).unwrap();
//! let encoded = encoder.encode(&yuv_frame).unwrap();
//! ```

use crate::core::{Result, TarangError, VideoFrame};
use openh264::Timestamp;
use openh264::formats::YUVSlices;

/// H.264 encoder configuration
#[derive(Debug, Clone)]
pub struct OpenH264EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub bitrate_bps: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
}

impl Default for OpenH264EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            bitrate_bps: 5_000_000,
            frame_rate_num: 30,
            frame_rate_den: 1,
        }
    }
}

/// H.264 encoder powered by openh264
pub struct OpenH264Encoder {
    encoder: openh264::encoder::Encoder,
    frames_encoded: u64,
    width: u32,
    height: u32,
}

impl OpenH264Encoder {
    pub fn new(config: &OpenH264EncoderConfig) -> Result<Self> {
        crate::core::validate_video_dimensions(config.width, config.height)?;

        let api = openh264::OpenH264API::from_source();
        let enc_config = openh264::encoder::EncoderConfig::new()
            .bitrate(openh264::encoder::BitRate::from_bps(config.bitrate_bps))
            .max_frame_rate(openh264::encoder::FrameRate::from_hz(
                config.frame_rate_num as f32 / config.frame_rate_den as f32,
            ));

        let encoder =
            openh264::encoder::Encoder::with_api_config(api, enc_config).map_err(|e| {
                TarangError::EncodeError(format!("openh264 encoder init failed: {e:?}").into())
            })?;

        Ok(Self {
            encoder,
            frames_encoded: 0,
            width: config.width,
            height: config.height,
        })
    }

    /// Encode a YUV420p frame. Returns encoded H.264 NAL units.
    pub fn encode(&mut self, frame: &VideoFrame) -> Result<Vec<u8>> {
        if frame.width != self.width || frame.height != self.height {
            return Err(TarangError::EncodeError(
                format!(
                    "frame dimensions {}x{} do not match encoder {}x{}",
                    frame.width, frame.height, self.width, self.height
                )
                .into(),
            ));
        }
        if frame.pixel_format != crate::core::PixelFormat::Yuv420p {
            return Err(TarangError::EncodeError(
                format!("expected YUV420p frame, got {:?}", frame.pixel_format).into(),
            ));
        }

        let w = self.width as usize;
        let h = self.height as usize;
        let y_size = w.checked_mul(h).ok_or_else(|| {
            TarangError::EncodeError("overflow computing Y plane size (w*h)".into())
        })?;
        // Floor division is safe here — dimensions are validated even in new()
        let chroma_w = w / 2;
        let chroma_h = h / 2;
        let chroma_size = chroma_w.checked_mul(chroma_h).ok_or_else(|| {
            TarangError::EncodeError("overflow computing chroma plane size".into())
        })?;
        let expected_size = y_size.checked_add(2 * chroma_size).ok_or_else(|| {
            TarangError::EncodeError("overflow computing total YUV420p frame size".into())
        })?;

        if frame.data.len() < expected_size {
            return Err(TarangError::EncodeError(
                format!(
                    "VideoFrame data too small: got {} bytes, expected {expected_size}",
                    frame.data.len()
                )
                .into(),
            ));
        }

        // Borrow slices from frame.data without copying
        let y_data = &frame.data[..y_size];
        let u_data = &frame.data[y_size..y_size + chroma_w * chroma_h];
        let v_data = &frame.data[y_size + chroma_w * chroma_h..expected_size];

        let yuv = YUVSlices::new((y_data, u_data, v_data), (w, h), (w, chroma_w, chroma_w));

        let ts = Timestamp::from_millis(frame.timestamp.as_millis() as u64);
        let bitstream = self
            .encoder
            .encode_at(&yuv, ts)
            .map_err(|e| TarangError::EncodeError(format!("openh264 encode: {e:?}").into()))?;

        let output = bitstream.to_vec();
        self.frames_encoded += 1;

        Ok(output)
    }

    pub fn frames_encoded(&self) -> u64 {
        self.frames_encoded
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::PixelFormat;
    use bytes::Bytes;
    use std::time::Duration;

    fn make_yuv420p_frame(width: u32, height: u32) -> VideoFrame {
        let y_size = (width * height) as usize;
        let chroma_w = (width / 2) as usize;
        let chroma_h = (height / 2) as usize;
        let total = y_size + 2 * chroma_w * chroma_h;
        // Gradient pattern for Y, flat 128 for U/V
        let mut data = vec![0u8; total];
        for (i, pixel) in data[..y_size].iter_mut().enumerate() {
            *pixel = (i % 256) as u8;
        }
        for pixel in &mut data[y_size..total] {
            *pixel = 128;
        }
        VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Yuv420p,
            width,
            height,
            timestamp: Duration::from_millis(33),
        }
    }

    #[test]
    fn encoder_creation() {
        let config = OpenH264EncoderConfig {
            width: 320,
            height: 240,
            ..Default::default()
        };
        let encoder = OpenH264Encoder::new(&config).unwrap();
        assert_eq!(encoder.dimensions(), (320, 240));
        assert_eq!(encoder.frames_encoded(), 0);
    }

    #[test]
    fn encoder_rejects_zero_dimensions() {
        let config = OpenH264EncoderConfig {
            width: 0,
            height: 240,
            ..Default::default()
        };
        assert!(OpenH264Encoder::new(&config).is_err());
    }

    #[test]
    fn encoder_rejects_odd_dimensions() {
        let config = OpenH264EncoderConfig {
            width: 321,
            height: 240,
            ..Default::default()
        };
        assert!(OpenH264Encoder::new(&config).is_err());
    }

    #[test]
    fn encode_single_frame() {
        let config = OpenH264EncoderConfig {
            width: 320,
            height: 240,
            ..Default::default()
        };
        let mut encoder = OpenH264Encoder::new(&config).unwrap();
        let frame = make_yuv420p_frame(320, 240);
        let output = encoder.encode(&frame).unwrap();
        assert!(!output.is_empty(), "encoder should produce output");
        assert_eq!(encoder.frames_encoded(), 1);
    }

    #[test]
    fn encode_multiple_frames() {
        let config = OpenH264EncoderConfig {
            width: 160,
            height: 120,
            ..Default::default()
        };
        let mut encoder = OpenH264Encoder::new(&config).unwrap();
        for i in 0..5 {
            let mut frame = make_yuv420p_frame(160, 120);
            frame.timestamp = Duration::from_millis(i * 33);
            encoder.encode(&frame).unwrap();
        }
        assert_eq!(encoder.frames_encoded(), 5);
    }

    #[test]
    fn encode_rejects_wrong_dimensions() {
        let config = OpenH264EncoderConfig {
            width: 320,
            height: 240,
            ..Default::default()
        };
        let mut encoder = OpenH264Encoder::new(&config).unwrap();
        let frame = make_yuv420p_frame(640, 480);
        assert!(encoder.encode(&frame).is_err());
    }

    #[test]
    fn encode_rejects_wrong_pixel_format() {
        let config = OpenH264EncoderConfig {
            width: 320,
            height: 240,
            ..Default::default()
        };
        let mut encoder = OpenH264Encoder::new(&config).unwrap();
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 320 * 240 * 3]),
            pixel_format: PixelFormat::Rgb24,
            width: 320,
            height: 240,
            timestamp: Duration::ZERO,
        };
        assert!(encoder.encode(&frame).is_err());
    }

    #[test]
    fn encode_rejects_short_data() {
        let config = OpenH264EncoderConfig {
            width: 320,
            height: 240,
            ..Default::default()
        };
        let mut encoder = OpenH264Encoder::new(&config).unwrap();
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 100]),
            pixel_format: PixelFormat::Yuv420p,
            width: 320,
            height: 240,
            timestamp: Duration::ZERO,
        };
        assert!(encoder.encode(&frame).is_err());
    }

    #[test]
    fn test_openh264_enc_frame_size_validation() {
        // Encoder should reject frames whose data length does not match
        // the expected YUV420p size for the configured dimensions.
        let config = OpenH264EncoderConfig {
            width: 320,
            height: 240,
            ..Default::default()
        };
        let mut encoder = OpenH264Encoder::new(&config).unwrap();

        // Expected: 320*240 + 2*(160*120) = 76800 + 38400 = 115200
        let expected = 320 * 240 + 2 * 160 * 120;

        // Too small by 1 byte
        let frame_small = VideoFrame {
            data: Bytes::from(vec![0u8; expected - 1]),
            pixel_format: PixelFormat::Yuv420p,
            width: 320,
            height: 240,
            timestamp: Duration::ZERO,
        };
        assert!(
            encoder.encode(&frame_small).is_err(),
            "data 1 byte too small should be rejected"
        );

        // Exactly the right size should succeed
        let frame_exact = make_yuv420p_frame(320, 240);
        assert!(
            encoder.encode(&frame_exact).is_ok(),
            "correctly-sized frame should be accepted"
        );

        // Wrong pixel format should fail
        let frame_rgb = VideoFrame {
            data: Bytes::from(vec![0u8; expected]),
            pixel_format: PixelFormat::Rgb24,
            width: 320,
            height: 240,
            timestamp: Duration::ZERO,
        };
        assert!(
            encoder.encode(&frame_rgb).is_err(),
            "RGB24 pixel format should be rejected"
        );
    }
}
