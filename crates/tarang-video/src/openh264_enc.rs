//! H.264 encoding via openh264 (Cisco BSD-2-Clause)
//!
//! Safe Rust wrapper around openh264 for H.264 encoding.
//! Requires the `openh264-enc` feature.

use openh264::formats::YUVSlices;
use openh264::Timestamp;
use tarang_core::{Result, TarangError, VideoFrame};

/// H.264 encoder configuration
#[derive(Debug, Clone)]
pub struct OpenH264EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub bitrate_bps: u32,
    pub frame_rate: f32,
}

impl Default for OpenH264EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            bitrate_bps: 5_000_000,
            frame_rate: 30.0,
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
        if config.width == 0 || config.height == 0 {
            return Err(TarangError::Pipeline(
                "OpenH264Encoder: width and height must be non-zero".to_string(),
            ));
        }
        if config.width % 2 != 0 || config.height % 2 != 0 {
            return Err(TarangError::Pipeline(format!(
                "OpenH264Encoder: dimensions must be even, got {}x{}",
                config.width, config.height
            )));
        }

        let api = openh264::OpenH264API::from_source();
        let enc_config = openh264::encoder::EncoderConfig::new()
            .set_bitrate_bps(config.bitrate_bps)
            .max_frame_rate(config.frame_rate);

        let encoder = openh264::encoder::Encoder::with_api_config(api, enc_config)
            .map_err(|e| TarangError::Pipeline(format!("openh264 encoder init failed: {e:?}")))?;

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
            return Err(TarangError::Pipeline(format!(
                "frame dimensions {}x{} do not match encoder {}x{}",
                frame.width, frame.height, self.width, self.height
            )));
        }

        let w = self.width as usize;
        let h = self.height as usize;
        let y_size = w * h;
        let chroma_w = w / 2;
        let chroma_h = h / 2;
        let expected_size = y_size + 2 * chroma_w * chroma_h;

        if frame.data.len() < expected_size {
            return Err(TarangError::Pipeline(format!(
                "VideoFrame data too small: got {} bytes, expected {expected_size}",
                frame.data.len()
            )));
        }

        // Borrow slices from frame.data without copying
        let y_data = &frame.data[..y_size];
        let u_data = &frame.data[y_size..y_size + chroma_w * chroma_h];
        let v_data = &frame.data[y_size + chroma_w * chroma_h..expected_size];

        let yuv = YUVSlices::new(
            (y_data, u_data, v_data),
            (w, h),
            (w, chroma_w, chroma_w),
        );

        let ts = Timestamp::from_millis(frame.timestamp.as_millis() as u64);
        let bitstream = self
            .encoder
            .encode_at(&yuv, ts)
            .map_err(|e| TarangError::Pipeline(format!("openh264 encode: {e:?}")))?;

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
