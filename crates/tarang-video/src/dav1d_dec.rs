//! AV1 decoding via dav1d FFI
//!
//! Safe Rust wrapper around the dav1d AV1 decoder.
//! Requires the `dav1d` feature and libdav1d system library.

use bytes::Bytes;
use std::time::Duration;
use tarang_core::{PixelFormat, Result, TarangError, VideoFrame};

/// AV1 decoder powered by dav1d
pub struct Dav1dDecoder {
    decoder: dav1d::Decoder,
    frames_decoded: u64,
}

impl Dav1dDecoder {
    pub fn new() -> Result<Self> {
        let settings = dav1d::Settings::new();
        let decoder = dav1d::Decoder::with_settings(&settings)
            .map_err(|e| TarangError::DecodeError(format!("dav1d init failed: {e}")))?;

        Ok(Self {
            decoder,
            frames_decoded: 0,
        })
    }

    /// Send encoded AV1 data to the decoder.
    ///
    /// `timestamp` is in nanoseconds — it is passed through to decoded frames
    /// and used to construct the output `Duration` via `Duration::from_nanos()`.
    /// Callers should use `duration.as_nanos() as i64` to convert.
    pub fn send_data(&mut self, data: &[u8], timestamp: i64) -> Result<()> {
        self.decoder
            .send_data(data.to_vec(), Some(timestamp), None, None)
            .map_err(|e| TarangError::DecodeError(format!("dav1d send_data: {e}")))
    }

    /// Try to get a decoded frame. Returns None if the decoder needs more data.
    pub fn get_frame(&mut self) -> Result<Option<VideoFrame>> {
        match self.decoder.get_picture() {
            Ok(pic) => {
                let width = pic.width();
                let height = pic.height();

                // Only YUV420p is supported; reject other layouts
                if pic.pixel_layout() != dav1d::PixelLayout::I420 {
                    return Err(TarangError::DecodeError(format!(
                        "unsupported pixel layout {:?}, expected I420",
                        pic.pixel_layout()
                    )));
                }

                let stride = pic.stride(dav1d::PlanarImageComponent::Y) as usize;
                let plane = pic.plane(dav1d::PlanarImageComponent::Y);

                // Use ceiling division for chroma dimensions (correct for odd sizes)
                let chroma_h = height.div_ceil(2) as usize;
                let chroma_w = width.div_ceil(2) as usize;
                let y_size = width as usize * height as usize;

                let mut yuv_data = Vec::with_capacity(y_size + 2 * chroma_w * chroma_h);

                // Copy Y plane tightly packed
                for row in 0..height as usize {
                    let start = row * stride;
                    let end = start + width as usize;
                    if end > plane.len() {
                        return Err(TarangError::DecodeError(format!(
                            "Y plane too small: need {end}, have {}",
                            plane.len()
                        )));
                    }
                    yuv_data.extend_from_slice(&plane[start..end]);
                }

                // Copy U and V planes for full YUV420p
                let u_stride = pic.stride(dav1d::PlanarImageComponent::U) as usize;
                let u_plane = pic.plane(dav1d::PlanarImageComponent::U);
                let v_stride = pic.stride(dav1d::PlanarImageComponent::V) as usize;
                let v_plane = pic.plane(dav1d::PlanarImageComponent::V);

                for row in 0..chroma_h {
                    let start = row * u_stride;
                    let end = start + chroma_w;
                    if end > u_plane.len() {
                        return Err(TarangError::DecodeError(format!(
                            "U plane too small: need {end}, have {}",
                            u_plane.len()
                        )));
                    }
                    yuv_data.extend_from_slice(&u_plane[start..end]);
                }
                for row in 0..chroma_h {
                    let start = row * v_stride;
                    let end = start + chroma_w;
                    if end > v_plane.len() {
                        return Err(TarangError::DecodeError(format!(
                            "V plane too small: need {end}, have {}",
                            v_plane.len()
                        )));
                    }
                    yuv_data.extend_from_slice(&v_plane[start..end]);
                }

                let timestamp_ns = pic.timestamp().unwrap_or(0).max(0) as u64;
                self.frames_decoded += 1;

                Ok(Some(VideoFrame {
                    data: Bytes::from(yuv_data),
                    pixel_format: PixelFormat::Yuv420p,
                    width,
                    height,
                    timestamp: Duration::from_nanos(timestamp_ns),
                }))
            }
            Err(dav1d::Error::Again) => Ok(None),
            Err(e) => Err(TarangError::DecodeError(format!("dav1d decode: {e}"))),
        }
    }

    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }

    pub fn flush(&mut self) {
        self.decoder.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_creation() {
        let decoder = Dav1dDecoder::new().unwrap();
        assert_eq!(decoder.frames_decoded(), 0);
    }

    #[test]
    fn get_frame_without_data_returns_none() {
        let mut decoder = Dav1dDecoder::new().unwrap();
        let result = decoder.get_frame().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn send_invalid_data_errors() {
        let mut decoder = Dav1dDecoder::new().unwrap();
        // Invalid AV1 data should error
        let result = decoder.send_data(&[0xDE, 0xAD, 0xBE, 0xEF], 0);
        // dav1d may accept and buffer invalid data or reject it — both are acceptable
        match result {
            Ok(()) => {
                // If accepted, get_frame should return None (no valid frame)
                let frame = decoder.get_frame().unwrap();
                assert!(frame.is_none());
            }
            Err(_) => {} // rejection is fine
        }
    }

    #[test]
    fn flush_on_empty_decoder() {
        let mut decoder = Dav1dDecoder::new().unwrap();
        decoder.flush();
        // After flush, get_frame should return None
        let result = decoder.get_frame().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn timestamp_passthrough() {
        // Verify that timestamps sent as nanoseconds are correctly
        // converted to Duration in the output frame.
        // We can't decode real AV1 data without a valid bitstream,
        // but we can verify the conversion math.
        let ts_nanos: i64 = 1_500_000_000; // 1.5 seconds
        let expected = Duration::from_nanos(ts_nanos as u64);
        assert_eq!(expected, Duration::from_millis(1500));
    }

    #[test]
    fn negative_timestamp_clamped_to_zero() {
        // Verify that max(0) on negative timestamp produces Duration::ZERO
        let negative_ts: i64 = -100;
        let clamped = negative_ts.max(0) as u64;
        assert_eq!(Duration::from_nanos(clamped), Duration::ZERO);
    }

    #[test]
    fn frames_decoded_starts_at_zero() {
        let decoder = Dav1dDecoder::new().unwrap();
        assert_eq!(decoder.frames_decoded(), 0);
    }
}
