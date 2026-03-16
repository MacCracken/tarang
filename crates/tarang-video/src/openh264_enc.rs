//! H.264 encoding via openh264 (Cisco BSD-2-Clause)
//!
//! Safe Rust wrapper around openh264 for H.264 encoding.
//! Requires the `openh264-enc` feature.

use openh264::formats::YUVBuffer;
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
        // YUVBuffer::from_vec expects tightly packed I420 [Y][U][V]
        let yuv = YUVBuffer::from_vec(
            frame.data.to_vec(),
            self.width as usize,
            self.height as usize,
        );

        let bitstream = self
            .encoder
            .encode(&yuv)
            .map_err(|e| TarangError::Pipeline(format!("openh264 encode: {e:?}")))?;

        let mut output = Vec::new();
        bitstream.write_vec(&mut output);
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
