//! Shared test helpers for video frame construction.

#[cfg(test)]
pub(crate) mod helpers {
    use crate::core::{PixelFormat, VideoFrame};
    use bytes::Bytes;
    use std::time::Duration;

    /// Create a solid YUV420p frame with uniform Y and neutral chroma (128).
    pub fn make_solid_yuv_frame(width: u32, height: u32, y_val: u8) -> VideoFrame {
        let y_size = (width * height) as usize;
        let chroma_w = width.div_ceil(2) as usize;
        let chroma_h = height.div_ceil(2) as usize;
        let mut data = vec![y_val; y_size];
        data.resize(y_size + 2 * chroma_w * chroma_h, 128);
        VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Yuv420p,
            width,
            height,
            timestamp: Duration::ZERO,
        }
    }

    /// Create a solid RGB24 frame with uniform color.
    pub fn make_solid_rgb_frame(width: u32, height: u32, r: u8, g: u8, b: u8) -> VideoFrame {
        let size = (width * height * 3) as usize;
        let mut data = Vec::with_capacity(size);
        for _ in 0..(width * height) {
            data.push(r);
            data.push(g);
            data.push(b);
        }
        VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Rgb24,
            width,
            height,
            timestamp: Duration::ZERO,
        }
    }
}
