//! Video frame scaling
//!
//! Resizes video frames using the `image` crate.
//! YUV420p frames are scaled directly per-plane (Y, U, V independently)
//! to avoid the YUV→RGB→scale→RGB→YUV roundtrip.
//!
//! ```rust,ignore
//! use tarang::video::scale::{scale_frame, ScaleFilter};
//!
//! let scaled = scale_frame(&frame, 1280, 720, ScaleFilter::Lanczos3).unwrap();
//! ```

use crate::core::{PixelFormat, Result, TarangError, VideoFrame};
use bytes::Bytes;
use image::{GrayImage, ImageBuffer, RgbImage};

/// Scaling filter algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleFilter {
    /// Nearest-neighbor (fastest, blocky).
    Nearest,
    /// Bilinear interpolation (good balance).
    Bilinear,
    /// Lanczos3 (sharpest, slowest).
    Lanczos3,
}

impl ScaleFilter {
    fn to_image_filter(self) -> image::imageops::FilterType {
        match self {
            ScaleFilter::Nearest => image::imageops::FilterType::Nearest,
            ScaleFilter::Bilinear => image::imageops::FilterType::Triangle,
            ScaleFilter::Lanczos3 => image::imageops::FilterType::Lanczos3,
        }
    }
}

/// Scale a video frame to the given dimensions.
///
/// For RGB24 frames, uses the `image` crate directly.
/// For YUV420p frames, scales Y/U/V planes independently as grayscale.
/// Other pixel formats are not currently supported.
pub fn scale_frame(
    frame: &VideoFrame,
    width: u32,
    height: u32,
    filter: ScaleFilter,
) -> Result<VideoFrame> {
    if width == 0 || height == 0 {
        return Err(TarangError::ConfigError(
            "scale target dimensions must be non-zero".into(),
        ));
    }

    // If dimensions already match, return a clone
    if frame.width == width && frame.height == height {
        return Ok(frame.clone());
    }

    match frame.pixel_format {
        PixelFormat::Rgb24 => scale_rgb24(frame, width, height, filter),
        PixelFormat::Yuv420p => scale_yuv420p_direct(frame, width, height, filter),
        other => Err(TarangError::ImageError(
            format!("scaling not supported for pixel format: {other:?}").into(),
        )),
    }
}

/// Scale a single grayscale plane using the `image` crate.
fn scale_plane(
    data: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    filter: ScaleFilter,
) -> Result<Vec<u8>> {
    let img: GrayImage = ImageBuffer::from_raw(src_w, src_h, data.to_vec()).ok_or_else(|| {
        TarangError::ImageError("failed to create grayscale image from plane data".into())
    })?;
    let resized = image::imageops::resize(&img, dst_w, dst_h, filter.to_image_filter());
    Ok(resized.into_raw())
}

/// Scale a YUV420p frame by scaling Y/U/V planes independently.
///
/// Avoids the YUV→RGB→scale→RGB→YUV roundtrip.
fn scale_yuv420p_direct(
    frame: &VideoFrame,
    width: u32,
    height: u32,
    filter: ScaleFilter,
) -> Result<VideoFrame> {
    let src_w = frame.width;
    let src_h = frame.height;
    let y_size = (src_w * src_h) as usize;
    let src_chroma_w = src_w.div_ceil(2);
    let src_chroma_h = src_h.div_ceil(2);
    let chroma_size = (src_chroma_w * src_chroma_h) as usize;

    if frame.data.len() < y_size + 2 * chroma_size {
        return Err(TarangError::ImageError("frame data too small".into()));
    }

    let y_plane = &frame.data[..y_size];
    let u_plane = &frame.data[y_size..y_size + chroma_size];
    let v_plane = &frame.data[y_size + chroma_size..y_size + 2 * chroma_size];

    let dst_chroma_w = width.div_ceil(2);
    let dst_chroma_h = height.div_ceil(2);

    let scaled_y = scale_plane(y_plane, src_w, src_h, width, height, filter)?;
    let scaled_u = scale_plane(
        u_plane,
        src_chroma_w,
        src_chroma_h,
        dst_chroma_w,
        dst_chroma_h,
        filter,
    )?;
    let scaled_v = scale_plane(
        v_plane,
        src_chroma_w,
        src_chroma_h,
        dst_chroma_w,
        dst_chroma_h,
        filter,
    )?;

    let mut data = Vec::with_capacity(scaled_y.len() + scaled_u.len() + scaled_v.len());
    data.extend_from_slice(&scaled_y);
    data.extend_from_slice(&scaled_u);
    data.extend_from_slice(&scaled_v);

    Ok(VideoFrame {
        data: Bytes::from(data),
        pixel_format: PixelFormat::Yuv420p,
        width,
        height,
        timestamp: frame.timestamp,
    })
}

/// Scale an RGB24 frame using the `image` crate.
fn scale_rgb24(
    frame: &VideoFrame,
    width: u32,
    height: u32,
    filter: ScaleFilter,
) -> Result<VideoFrame> {
    let src_img: RgbImage = ImageBuffer::from_raw(frame.width, frame.height, frame.data.to_vec())
        .ok_or_else(|| {
        TarangError::ImageError("failed to create image buffer from RGB24 data".into())
    })?;

    let resized = image::imageops::resize(&src_img, width, height, filter.to_image_filter());

    Ok(VideoFrame {
        data: Bytes::from(resized.into_raw()),
        pixel_format: PixelFormat::Rgb24,
        width,
        height,
        timestamp: frame.timestamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_solid_rgb_frame(width: u32, height: u32, r: u8, g: u8, b: u8) -> VideoFrame {
        let mut data = Vec::with_capacity((width * height * 3) as usize);
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

    fn make_solid_yuv_frame(width: u32, height: u32, y_val: u8) -> VideoFrame {
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

    #[test]
    fn scale_rgb24_up() {
        let frame = make_solid_rgb_frame(4, 4, 128, 128, 128);
        let scaled = scale_frame(&frame, 8, 8, ScaleFilter::Bilinear).unwrap();
        assert_eq!(scaled.width, 8);
        assert_eq!(scaled.height, 8);
        assert_eq!(scaled.pixel_format, PixelFormat::Rgb24);
        assert_eq!(scaled.data.len(), 8 * 8 * 3);
    }

    #[test]
    fn scale_rgb24_down() {
        let frame = make_solid_rgb_frame(16, 16, 200, 100, 50);
        let scaled = scale_frame(&frame, 4, 4, ScaleFilter::Lanczos3).unwrap();
        assert_eq!(scaled.width, 4);
        assert_eq!(scaled.height, 4);
        assert_eq!(scaled.data.len(), 4 * 4 * 3);
    }

    #[test]
    fn scale_rgb24_nearest() {
        let frame = make_solid_rgb_frame(4, 4, 255, 0, 0);
        let scaled = scale_frame(&frame, 8, 8, ScaleFilter::Nearest).unwrap();
        assert_eq!(scaled.width, 8);
        assert_eq!(scaled.height, 8);
        // Nearest-neighbor on solid color should preserve the color
        assert_eq!(scaled.data[0], 255);
        assert_eq!(scaled.data[1], 0);
        assert_eq!(scaled.data[2], 0);
    }

    #[test]
    fn scale_yuv420p_up() {
        let frame = make_solid_yuv_frame(4, 4, 128);
        let scaled = scale_frame(&frame, 8, 8, ScaleFilter::Bilinear).unwrap();
        assert_eq!(scaled.width, 8);
        assert_eq!(scaled.height, 8);
        assert_eq!(scaled.pixel_format, PixelFormat::Yuv420p);
    }

    #[test]
    fn scale_yuv420p_down() {
        let frame = make_solid_yuv_frame(16, 16, 128);
        let scaled = scale_frame(&frame, 4, 4, ScaleFilter::Bilinear).unwrap();
        assert_eq!(scaled.width, 4);
        assert_eq!(scaled.height, 4);
        assert_eq!(scaled.pixel_format, PixelFormat::Yuv420p);
    }

    #[test]
    fn scale_identity() {
        let frame = make_solid_rgb_frame(8, 8, 42, 42, 42);
        let scaled = scale_frame(&frame, 8, 8, ScaleFilter::Bilinear).unwrap();
        assert_eq!(scaled.data, frame.data);
    }

    #[test]
    fn scale_zero_dimensions_error() {
        let frame = make_solid_rgb_frame(4, 4, 128, 128, 128);
        assert!(scale_frame(&frame, 0, 4, ScaleFilter::Bilinear).is_err());
        assert!(scale_frame(&frame, 4, 0, ScaleFilter::Bilinear).is_err());
    }

    #[test]
    fn scale_unsupported_format() {
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 100]),
            pixel_format: PixelFormat::Rgba32,
            width: 5,
            height: 5,
            timestamp: Duration::ZERO,
        };
        assert!(scale_frame(&frame, 10, 10, ScaleFilter::Bilinear).is_err());
    }

    #[test]
    fn scale_preserves_timestamp() {
        let mut frame = make_solid_rgb_frame(4, 4, 128, 128, 128);
        frame.timestamp = Duration::from_millis(999);
        let scaled = scale_frame(&frame, 8, 8, ScaleFilter::Bilinear).unwrap();
        assert_eq!(scaled.timestamp, Duration::from_millis(999));
    }
}
