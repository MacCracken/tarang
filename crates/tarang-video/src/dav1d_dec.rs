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
    /// The `timestamp` value is opaque — it is passed through to decoded frames
    /// and interpreted as nanoseconds when constructing the output `Duration`.
    pub fn send_data(&mut self, data: &[u8], timestamp: i64) -> Result<()> {
        self.decoder
            .send_data(data.to_vec(), Some(timestamp), None, None)
            .map_err(|e| TarangError::DecodeError(format!("dav1d send_data: {e}")))
    }

    /// Try to get a decoded frame. Returns None if the decoder needs more data.
    pub fn get_frame(&mut self) -> Result<Option<VideoFrame>> {
        match self.decoder.get_picture() {
            Ok(pic) => {
                let width = pic.width() as u32;
                let height = pic.height() as u32;

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
                let chroma_h = ((height + 1) / 2) as usize;
                let chroma_w = ((width + 1) / 2) as usize;
                let y_size = width as usize * height as usize;

                let mut yuv_data = Vec::with_capacity(y_size + 2 * chroma_w * chroma_h);

                // Copy Y plane tightly packed
                for row in 0..height as usize {
                    let start = row * stride;
                    let end = start + width as usize;
                    yuv_data.extend_from_slice(&plane[start..end]);
                }

                // Copy U and V planes for full YUV420p
                let u_stride = pic.stride(dav1d::PlanarImageComponent::U) as usize;
                let u_plane = pic.plane(dav1d::PlanarImageComponent::U);
                let v_stride = pic.stride(dav1d::PlanarImageComponent::V) as usize;
                let v_plane = pic.plane(dav1d::PlanarImageComponent::V);

                for row in 0..chroma_h {
                    let start = row * u_stride;
                    yuv_data.extend_from_slice(&u_plane[start..start + chroma_w]);
                }
                for row in 0..chroma_h {
                    let start = row * v_stride;
                    yuv_data.extend_from_slice(&v_plane[start..start + chroma_w]);
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
