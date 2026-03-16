//! H.264 decoding via openh264 (Cisco BSD-2-Clause)
//!
//! Safe Rust wrapper around openh264 for H.264 decoding.
//! Requires the `openh264` feature.

use bytes::Bytes;
use openh264::formats::YUVSource;
use std::time::Duration;
use tarang_core::{PixelFormat, Result, TarangError, VideoFrame};

/// H.264 decoder powered by openh264
pub struct OpenH264Decoder {
    decoder: openh264::decoder::Decoder,
    frames_decoded: u64,
}

impl OpenH264Decoder {
    pub fn new() -> Result<Self> {
        let api = openh264::OpenH264API::from_source();
        let config = openh264::decoder::DecoderConfig::new();
        let decoder = openh264::decoder::Decoder::with_api_config(api, config)
            .map_err(|e| TarangError::DecodeError(format!("openh264 init failed: {e:?}")))?;

        Ok(Self {
            decoder,
            frames_decoded: 0,
        })
    }

    /// Decode an H.264 NAL unit. Returns None if the decoder needs more data.
    pub fn decode(&mut self, data: &[u8], timestamp: Duration) -> Result<Option<VideoFrame>> {
        let decoded = self
            .decoder
            .decode(data)
            .map_err(|e| TarangError::DecodeError(format!("openh264 decode: {e:?}")))?;

        let Some(yuv) = decoded else {
            return Ok(None);
        };

        let (width, height) = yuv.dimensions();
        let width = width as u32;
        let height = height as u32;
        let (y_stride, u_stride, v_stride) = yuv.strides();

        // Extract YUV420p planes tightly packed
        let chroma_w = (width / 2) as usize;
        let chroma_h = (height / 2) as usize;
        let y_size = (width * height) as usize;
        let total_size = y_size + chroma_w * chroma_h * 2;
        let mut yuv_data = Vec::with_capacity(total_size);

        // Y plane
        let y_plane = yuv.y();
        for row in 0..height as usize {
            let start = row * y_stride;
            let end = start + width as usize;
            yuv_data.extend_from_slice(&y_plane[start..end]);
        }

        // U plane
        let u_plane = yuv.u();
        for row in 0..chroma_h {
            let start = row * u_stride;
            yuv_data.extend_from_slice(&u_plane[start..start + chroma_w]);
        }

        // V plane
        let v_plane = yuv.v();
        for row in 0..chroma_h {
            let start = row * v_stride;
            yuv_data.extend_from_slice(&v_plane[start..start + chroma_w]);
        }

        self.frames_decoded += 1;

        Ok(Some(VideoFrame {
            data: Bytes::from(yuv_data),
            pixel_format: PixelFormat::Yuv420p,
            width,
            height,
            timestamp,
        }))
    }

    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }
}
