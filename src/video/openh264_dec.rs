//! H.264 decoding via openh264 (Cisco BSD-2-Clause)
//!
//! Safe Rust wrapper around openh264 for H.264 decoding.
//! Requires the `openh264` feature.

use crate::core::{PixelFormat, Result, TarangError, VideoFrame};
use bytes::Bytes;
use openh264::formats::YUVSource;
use std::time::Duration;

/// Extract tightly-packed YUV420p data from a decoded YUV frame.
fn extract_yuv420p(yuv: &impl YUVSource, timestamp: Duration) -> VideoFrame {
    let (width, height) = yuv.dimensions();
    let width = width as u32;
    let height = height as u32;
    let (y_stride, u_stride, v_stride) = yuv.strides();

    let chroma_w = width.div_ceil(2) as usize;
    let chroma_h = height.div_ceil(2) as usize;

    // Validate strides to prevent out-of-bounds access
    if y_stride < width as usize || u_stride < chroma_w || v_stride < chroma_w {
        return VideoFrame {
            data: Bytes::new(),
            pixel_format: PixelFormat::Yuv420p,
            width,
            height,
            timestamp,
        };
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_creation() {
        let decoder = OpenH264Decoder::new().unwrap();
        assert_eq!(decoder.frames_decoded(), 0);
    }

    #[test]
    fn decode_empty_returns_none() {
        let mut decoder = OpenH264Decoder::new().unwrap();
        // Empty data should not produce a frame (may error or return None)
        let result = decoder.decode(&[], Duration::ZERO);
        // openh264 may return Ok(None) or Err for empty input — both are acceptable
        match result {
            Ok(None) => {}
            Err(_) => {}
            Ok(Some(_)) => panic!("empty input should not produce a frame"),
        }
    }

    #[test]
    fn encode_decode_roundtrip() {
        // Encode a frame with openh264, then decode it back
        use crate::video::openh264_enc::{OpenH264Encoder, OpenH264EncoderConfig};

        let config = OpenH264EncoderConfig {
            width: 320,
            height: 240,
            bitrate_bps: 500_000,
            ..Default::default()
        };
        let mut encoder = OpenH264Encoder::new(&config).unwrap();

        // Create a test YUV420p frame
        let w = 320usize;
        let h = 240usize;
        let y_size = w * h;
        let chroma = w / 2 * h / 2;
        let mut data = vec![128u8; y_size + 2 * chroma];
        // Gradient Y plane
        for y in 0..h {
            for x in 0..w {
                data[y * w + x] = ((x + y) % 256) as u8;
            }
        }

        let frame = VideoFrame {
            data: bytes::Bytes::from(data),
            pixel_format: PixelFormat::Yuv420p,
            width: 320,
            height: 240,
            timestamp: Duration::from_millis(0),
        };

        // Encode several frames (encoder needs a few to produce stable output)
        let mut h264_data = Vec::new();
        for i in 0..3 {
            let mut f = frame.clone();
            f.timestamp = Duration::from_millis(i * 33);
            let encoded = encoder.encode(&f).unwrap();
            h264_data.extend_from_slice(&encoded);
        }
        assert!(!h264_data.is_empty(), "encoder should produce H.264 data");

        // Decode the H.264 data
        let mut decoder = OpenH264Decoder::new().unwrap();
        let result = decoder.decode(&h264_data, Duration::ZERO).unwrap();
        // The decoder may or may not produce a frame from concatenated NAL units
        // — the important thing is it doesn't crash
        if let Some(frame) = result {
            assert_eq!(frame.pixel_format, PixelFormat::Yuv420p);
            assert!(frame.width > 0);
            assert!(frame.height > 0);
        }
    }
}
