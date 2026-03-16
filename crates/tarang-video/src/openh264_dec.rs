//! H.264 decoding via openh264 (Cisco BSD-2-Clause)
//!
//! Safe Rust wrapper around openh264 for H.264 decoding.
//! Requires the `openh264` feature.

use bytes::Bytes;
use openh264::formats::YUVSource;
use std::time::Duration;
use tarang_core::{PixelFormat, Result, TarangError, VideoFrame};

/// Extract tightly-packed YUV420p data from a decoded YUV frame.
fn extract_yuv420p(yuv: &impl YUVSource, timestamp: Duration) -> VideoFrame {
    let (width, height) = yuv.dimensions();
    let width = width as u32;
    let height = height as u32;
    let (y_stride, u_stride, v_stride) = yuv.strides();

    let chroma_w = ((width + 1) / 2) as usize;
    let chroma_h = ((height + 1) / 2) as usize;
    let y_size = width as usize * height as usize;
    let mut yuv_data = Vec::with_capacity(y_size + chroma_w * chroma_h * 2);

    let y_plane = yuv.y();
    for row in 0..height as usize {
        let start = row * y_stride;
        yuv_data.extend_from_slice(&y_plane[start..start + width as usize]);
    }

    let u_plane = yuv.u();
    for row in 0..chroma_h {
        let start = row * u_stride;
        yuv_data.extend_from_slice(&u_plane[start..start + chroma_w]);
    }

    let v_plane = yuv.v();
    for row in 0..chroma_h {
        let start = row * v_stride;
        yuv_data.extend_from_slice(&v_plane[start..start + chroma_w]);
    }

    VideoFrame {
        data: Bytes::from(yuv_data),
        pixel_format: PixelFormat::Yuv420p,
        width,
        height,
        timestamp,
    }
}

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

        self.frames_decoded += 1;
        Ok(Some(extract_yuv420p(&yuv, timestamp)))
    }

    /// Flush remaining buffered frames from the decoder.
    pub fn flush(&mut self) -> Result<Vec<VideoFrame>> {
        let remaining = self
            .decoder
            .flush_remaining()
            .map_err(|e| TarangError::DecodeError(format!("openh264 flush: {e:?}")))?;

        let mut frames = Vec::new();
        for yuv in &remaining {
            let ts_ms = yuv.timestamp().as_millis();
            let timestamp = Duration::from_millis(ts_ms);
            self.frames_decoded += 1;
            frames.push(extract_yuv420p(yuv, timestamp));
        }

        Ok(frames)
    }

    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }
}
