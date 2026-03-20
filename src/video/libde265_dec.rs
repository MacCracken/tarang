// WARNING: This module links against libde265, which is LGPL-3.0-only.
// Tarang is GPL-3.0, which is compatible, but downstream consumers with
// more permissive licenses (MIT, Apache-2.0, BSD) CANNOT use this feature.
//
// This module is behind the `h265-decode` feature flag and is deliberately
// NOT included in the `full` feature.
//
// TO REMOVE THIS MODULE:
//   1. Delete this file (src/video/libde265_dec.rs)
//   2. Remove `h265-decode` from Cargo.toml [features]
//   3. Remove `libde265-rs` from Cargo.toml [dependencies]
//   4. Remove the `mod libde265_dec` and `pub use` lines in src/video/mod.rs
//   5. Remove "LGPL-3.0-only" from deny.toml [licenses].allow

//! H.265/HEVC software decoding via libde265 (LGPL-3.0)
//!
//! **License warning**: libde265 is LGPL-3.0-only. This module is opt-in
//! via the `h265-decode` feature and excluded from `full`.
//!
//! Provides software H.265 decode for environments without VA-API hardware
//! (CI, headless servers, macOS). For GPU-accelerated decode, use
//! `DecoderConfig::for_codec_auto()` with the `hwaccel` + `vaapi` features.

use crate::core::{PixelFormat, Result, TarangError, VideoFrame};
use bytes::Bytes;
use std::time::Duration;

/// H.265/HEVC software decoder backed by libde265 (LGPL-3.0).
///
/// **License**: LGPL-3.0-only. See module-level warning.
pub struct LibDe265Decoder {
    input: libde265_rs::DecoderInput,
    output: libde265_rs::DecoderOutput,
    frames_decoded: u64,
}

impl LibDe265Decoder {
    /// Create a new H.265 software decoder.
    pub fn new() -> Result<Self> {
        let (input, output) = libde265_rs::new_decoder()
            .map_err(|e| TarangError::DecodeError(format!("libde265 init: {e}").into()))?;
        Ok(Self {
            input,
            output,
            frames_decoded: 0,
        })
    }

    /// Start background worker threads for parallel slice decoding.
    pub fn start_threads(&mut self, count: u32) -> Result<()> {
        self.input
            .start_worker_threads(count)
            .map_err(|e| TarangError::DecodeError(format!("libde265 thread init: {e}").into()))
    }

    /// Push raw H.265 bitstream data (Annex B or NAL units).
    pub fn push_data(&mut self, data: &[u8], timestamp: Duration) -> Result<()> {
        let pts = timestamp.as_micros() as i64;
        self.input
            .push_data(data, pts, 0)
            .map_err(|e| TarangError::DecodeError(format!("libde265 push_data: {e}").into()))
    }

    /// Push a single NAL unit.
    pub fn push_nal(&mut self, data: &[u8], timestamp: Duration) -> Result<()> {
        let pts = timestamp.as_micros() as i64;
        self.input
            .push_nal(data, pts, 0)
            .map_err(|e| TarangError::DecodeError(format!("libde265 push_nal: {e}").into()))
    }

    /// Drive the decoder — call after pushing data.
    /// Returns `true` if more data is needed, `false` when done.
    pub fn decode(&mut self) -> Result<bool> {
        match self.input.decode() {
            Ok(libde265_rs::DecodeResult::Done) => Ok(false),
            Ok(_) => Ok(true),
            Err(e) => Err(TarangError::DecodeError(
                format!("libde265 decode: {e}").into(),
            )),
        }
    }

    /// Signal end of stream — flush remaining frames.
    pub fn flush(&mut self) -> Result<()> {
        self.input
            .flush_data()
            .map_err(|e| TarangError::DecodeError(format!("libde265 flush: {e}").into()))
    }

    /// Get the next decoded frame, if available.
    ///
    /// Returns a YUV420p `VideoFrame` or `None` if no frame is ready.
    pub fn next_frame(&mut self) -> Option<VideoFrame> {
        let image = self.output.next_picture()?;

        let w = image.width(libde265_rs::Channel::Y) as u32;
        let h = image.height(libde265_rs::Channel::Y) as u32;
        let pts_us = image.pts();
        let timestamp = Duration::from_micros(pts_us.max(0) as u64);

        // Extract YUV planes and pack into contiguous YUV420p layout
        let (y_plane, y_stride) = image.plane(libde265_rs::Channel::Y);
        let (cb_plane, cb_stride) = image.plane(libde265_rs::Channel::Cb);
        let (cr_plane, cr_stride) = image.plane(libde265_rs::Channel::Cr);

        let chroma_w = (w as usize + 1) / 2;
        let chroma_h = (h as usize + 1) / 2;
        let y_size = w as usize * h as usize;
        let uv_size = chroma_w * chroma_h;

        let mut data = Vec::with_capacity(y_size + 2 * uv_size);

        // Copy Y plane (handle stride != width)
        for row in 0..h as usize {
            let start = row * y_stride;
            data.extend_from_slice(&y_plane[start..start + w as usize]);
        }

        // Copy Cb (U) plane
        for row in 0..chroma_h {
            let start = row * cb_stride;
            data.extend_from_slice(&cb_plane[start..start + chroma_w]);
        }

        // Copy Cr (V) plane
        for row in 0..chroma_h {
            let start = row * cr_stride;
            data.extend_from_slice(&cr_plane[start..start + chroma_w]);
        }

        self.frames_decoded += 1;

        Some(VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Yuv420p,
            width: w,
            height: h,
            timestamp,
        })
    }

    /// Total frames decoded so far.
    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_creation() {
        let dec = LibDe265Decoder::new();
        assert!(dec.is_ok());
        assert_eq!(dec.unwrap().frames_decoded(), 0);
    }

    #[test]
    fn decoder_with_threads() {
        let mut dec = LibDe265Decoder::new().unwrap();
        // Starting threads should succeed
        assert!(dec.start_threads(2).is_ok());
    }

    #[test]
    fn decode_empty_produces_no_frames() {
        let mut dec = LibDe265Decoder::new().unwrap();
        dec.flush().unwrap();
        // Drive decoder
        loop {
            match dec.decode() {
                Ok(true) => continue,
                _ => break,
            }
        }
        assert!(dec.next_frame().is_none());
    }

    #[test]
    fn push_invalid_data_does_not_panic() {
        let mut dec = LibDe265Decoder::new().unwrap();
        // Push garbage — should not panic
        let _ = dec.push_data(&[0xFF; 100], Duration::ZERO);
        let _ = dec.decode();
        // No valid frame expected
        assert!(dec.next_frame().is_none());
    }
}
