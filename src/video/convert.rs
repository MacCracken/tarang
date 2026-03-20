//! Pixel format conversion for video frames
//!
//! Centralizes YUV420p, NV12, and RGB24 conversions using BT.601 coefficients.
//!
//! ```rust,ignore
//! use tarang::video::convert::convert_pixel_format;
//! use tarang::core::PixelFormat;
//!
//! let rgb_frame = convert_pixel_format(&yuv_frame, PixelFormat::Rgb24).unwrap();
//! ```

use crate::core::{PixelFormat, Result, TarangError, VideoFrame};
use bytes::Bytes;

/// Convert YUV420p frame data to RGB24.
///
/// Uses integer BT.601 math:
///   R = Y + (359*V >> 8)
///   G = Y - (88*U + 183*V >> 8)
///   B = Y + (454*U >> 8)
pub fn yuv420p_to_rgb24(frame: &VideoFrame) -> Result<VideoFrame> {
    if frame.pixel_format == PixelFormat::Rgb24 {
        return Ok(frame.clone());
    }
    if frame.pixel_format != PixelFormat::Yuv420p {
        return Err(TarangError::ImageError(
            format!(
                "unsupported pixel format for RGB conversion: {:?}",
                frame.pixel_format
            )
            .into(),
        ));
    }

    let w = frame.width as usize;
    let h = frame.height as usize;
    let y_size = w * h;
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);

    if frame.data.len() < y_size + 2 * chroma_w * chroma_h {
        return Err(TarangError::ImageError("frame data too small".into()));
    }

    let y_plane = &frame.data[..y_size];
    let u_plane = &frame.data[y_size..y_size + chroma_w * chroma_h];
    let v_plane = &frame.data[y_size + chroma_w * chroma_h..];

    let mut rgb = vec![0u8; w * h * 3];

    for chroma_row in 0..chroma_h {
        let chroma_row_offset = chroma_row * chroma_w;
        let mut cr_r = vec![0i32; chroma_w];
        let mut cr_g = vec![0i32; chroma_w];
        let mut cr_b = vec![0i32; chroma_w];

        for cx in 0..chroma_w {
            let u = u_plane[chroma_row_offset + cx] as i32 - 128;
            let v = v_plane[chroma_row_offset + cx] as i32 - 128;
            cr_r[cx] = (359 * v) >> 8;
            cr_g[cx] = (88 * u + 183 * v) >> 8;
            cr_b[cx] = (454 * u) >> 8;
        }

        let luma_row_start = chroma_row * 2;
        let luma_row_end = (luma_row_start + 2).min(h);

        for row in luma_row_start..luma_row_end {
            let y_row_offset = row * w;
            let rgb_row_offset = y_row_offset * 3;

            for col in 0..w {
                let y_val = y_plane[y_row_offset + col] as i32;
                let cx = col / 2;

                let r = (y_val + cr_r[cx]).clamp(0, 255) as u8;
                let g = (y_val - cr_g[cx]).clamp(0, 255) as u8;
                let b = (y_val + cr_b[cx]).clamp(0, 255) as u8;

                let offset = rgb_row_offset + col * 3;
                rgb[offset] = r;
                rgb[offset + 1] = g;
                rgb[offset + 2] = b;
            }
        }
    }

    Ok(VideoFrame {
        data: Bytes::from(rgb),
        pixel_format: PixelFormat::Rgb24,
        width: frame.width,
        height: frame.height,
        timestamp: frame.timestamp,
    })
}

/// Convert RGB24 frame data to YUV420p using BT.601 coefficients.
///
/// Inverse of `yuv420p_to_rgb24`:
///   Y  =  (66*R + 129*G + 25*B + 128) >> 8 + 16
///   U  = (-38*R - 74*G + 112*B + 128) >> 8 + 128
///   V  = (112*R - 94*G - 18*B + 128) >> 8 + 128
///
/// Uses full-range input (0..255) mapped to TV-range Y (16..235) and UV (16..240).
pub fn rgb24_to_yuv420p(frame: &VideoFrame) -> Result<VideoFrame> {
    if frame.pixel_format == PixelFormat::Yuv420p {
        return Ok(frame.clone());
    }
    if frame.pixel_format != PixelFormat::Rgb24 {
        return Err(TarangError::ImageError(
            format!(
                "unsupported pixel format for YUV conversion: {:?}",
                frame.pixel_format
            )
            .into(),
        ));
    }

    let w = frame.width as usize;
    let h = frame.height as usize;

    if frame.data.len() < w * h * 3 {
        return Err(TarangError::ImageError("frame data too small".into()));
    }

    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let y_size = w * h;

    let mut yuv = vec![0u8; y_size + 2 * chroma_w * chroma_h];
    let (y_plane, uv_planes) = yuv.split_at_mut(y_size);
    let (u_plane, v_plane) = uv_planes.split_at_mut(chroma_w * chroma_h);

    let rgb = &frame.data;

    // Compute Y for every pixel
    for row in 0..h {
        for col in 0..w {
            let offset = (row * w + col) * 3;
            let r = rgb[offset] as i32;
            let g = rgb[offset + 1] as i32;
            let b = rgb[offset + 2] as i32;

            let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            y_plane[row * w + col] = y.clamp(0, 255) as u8;
        }
    }

    // Compute U and V by averaging each 2x2 block
    for cy in 0..chroma_h {
        for cx in 0..chroma_w {
            let mut sum_r: i32 = 0;
            let mut sum_g: i32 = 0;
            let mut sum_b: i32 = 0;
            let mut count: i32 = 0;

            for dy in 0..2 {
                let row = cy * 2 + dy;
                if row >= h {
                    continue;
                }
                for dx in 0..2 {
                    let col = cx * 2 + dx;
                    if col >= w {
                        continue;
                    }
                    let offset = (row * w + col) * 3;
                    sum_r += rgb[offset] as i32;
                    sum_g += rgb[offset + 1] as i32;
                    sum_b += rgb[offset + 2] as i32;
                    count += 1;
                }
            }

            let r = sum_r / count;
            let g = sum_g / count;
            let b = sum_b / count;

            let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
            let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;

            let idx = cy * chroma_w + cx;
            u_plane[idx] = u.clamp(0, 255) as u8;
            v_plane[idx] = v.clamp(0, 255) as u8;
        }
    }

    Ok(VideoFrame {
        data: Bytes::from(yuv),
        pixel_format: PixelFormat::Yuv420p,
        width: frame.width,
        height: frame.height,
        timestamp: frame.timestamp,
    })
}

/// Convert YUV420p frame data to NV12 layout.
///
/// YUV420p: Y plane, U plane (w/2 * h/2), V plane (w/2 * h/2)
/// NV12:    Y plane, interleaved UV plane (w * h/2)
pub fn yuv420p_to_nv12(frame: &VideoFrame) -> Result<VideoFrame> {
    if frame.pixel_format == PixelFormat::Nv12 {
        return Ok(frame.clone());
    }
    if frame.pixel_format != PixelFormat::Yuv420p {
        return Err(TarangError::ImageError(
            format!(
                "unsupported pixel format for NV12 conversion: {:?}",
                frame.pixel_format
            )
            .into(),
        ));
    }

    let w = frame.width as usize;
    let h = frame.height as usize;
    let y_size = w * h;
    let uv_size = w * (h / 2);

    let mut nv12 = vec![0u8; y_size + uv_size];

    // Copy Y plane as-is
    nv12[..y_size].copy_from_slice(&frame.data[..y_size]);

    // Interleave U and V into NV12 UV plane
    let u_plane = &frame.data[y_size..y_size + y_size / 4];
    let v_plane = &frame.data[y_size + y_size / 4..];
    let uv_dst = &mut nv12[y_size..];
    for i in 0..y_size / 4 {
        uv_dst[i * 2] = u_plane[i];
        uv_dst[i * 2 + 1] = v_plane[i];
    }

    Ok(VideoFrame {
        data: Bytes::from(nv12),
        pixel_format: PixelFormat::Nv12,
        width: frame.width,
        height: frame.height,
        timestamp: frame.timestamp,
    })
}

/// Convert a video frame to the target pixel format.
///
/// Currently supports:
/// - YUV420p -> RGB24
/// - YUV420p -> NV12
/// - RGB24 -> YUV420p
/// - Identity (same format returns clone)
pub fn convert_pixel_format(frame: &VideoFrame, target: PixelFormat) -> Result<VideoFrame> {
    if frame.pixel_format == target {
        return Ok(frame.clone());
    }

    match (frame.pixel_format, target) {
        (PixelFormat::Yuv420p, PixelFormat::Rgb24) => yuv420p_to_rgb24(frame),
        (PixelFormat::Yuv420p, PixelFormat::Nv12) => yuv420p_to_nv12(frame),
        (PixelFormat::Rgb24, PixelFormat::Yuv420p) => rgb24_to_yuv420p(frame),
        (PixelFormat::Rgb24, PixelFormat::Nv12) => {
            let yuv = rgb24_to_yuv420p(frame)?;
            yuv420p_to_nv12(&yuv)
        }
        (src, dst) => Err(TarangError::ImageError(
            format!("unsupported conversion: {src:?} -> {dst:?}").into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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

    fn make_solid_rgb_frame(width: u32, height: u32, r: u8, g: u8, b: u8) -> VideoFrame {
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

    #[test]
    fn yuv420p_to_rgb24_neutral_gray() {
        let frame = make_solid_yuv_frame(4, 4, 128);
        let rgb = yuv420p_to_rgb24(&frame).unwrap();
        assert_eq!(rgb.pixel_format, PixelFormat::Rgb24);
        assert_eq!(rgb.data.len(), 4 * 4 * 3);
        assert!((rgb.data[0] as i32 - 128).abs() < 3);
        assert!((rgb.data[1] as i32 - 128).abs() < 3);
        assert!((rgb.data[2] as i32 - 128).abs() < 3);
    }

    #[test]
    fn yuv420p_to_rgb24_passthrough() {
        let frame = make_solid_rgb_frame(4, 4, 100, 150, 200);
        let result = yuv420p_to_rgb24(&frame).unwrap();
        assert_eq!(result.data, frame.data);
    }

    #[test]
    fn yuv420p_to_rgb24_rejects_unsupported() {
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 100]),
            pixel_format: PixelFormat::Yuv422p,
            width: 10,
            height: 10,
            timestamp: Duration::ZERO,
        };
        assert!(yuv420p_to_rgb24(&frame).is_err());
    }

    #[test]
    fn yuv420p_to_rgb24_too_small() {
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 10]),
            pixel_format: PixelFormat::Yuv420p,
            width: 64,
            height: 64,
            timestamp: Duration::ZERO,
        };
        assert!(yuv420p_to_rgb24(&frame).is_err());
    }

    #[test]
    fn rgb24_to_yuv420p_passthrough() {
        let frame = make_solid_yuv_frame(4, 4, 128);
        let result = rgb24_to_yuv420p(&frame).unwrap();
        assert_eq!(result.data, frame.data);
    }

    #[test]
    fn rgb24_to_yuv420p_rejects_unsupported() {
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 100]),
            pixel_format: PixelFormat::Nv12,
            width: 10,
            height: 10,
            timestamp: Duration::ZERO,
        };
        assert!(rgb24_to_yuv420p(&frame).is_err());
    }

    #[test]
    fn yuv420p_rgb24_roundtrip() {
        // Create a YUV420p frame with known values
        let frame = make_solid_yuv_frame(8, 8, 128);

        // Convert YUV -> RGB -> YUV
        let rgb = yuv420p_to_rgb24(&frame).unwrap();
        assert_eq!(rgb.pixel_format, PixelFormat::Rgb24);

        let yuv_back = rgb24_to_yuv420p(&rgb).unwrap();
        assert_eq!(yuv_back.pixel_format, PixelFormat::Yuv420p);
        assert_eq!(yuv_back.width, frame.width);
        assert_eq!(yuv_back.height, frame.height);

        // Y values should be close (within rounding tolerance)
        let y_size = (frame.width * frame.height) as usize;
        for i in 0..y_size {
            let orig = frame.data[i] as i32;
            let roundtrip = yuv_back.data[i] as i32;
            assert!(
                (orig - roundtrip).abs() <= 2,
                "Y[{i}]: orig={orig}, roundtrip={roundtrip}"
            );
        }
    }

    #[test]
    fn yuv420p_to_nv12_basic() {
        let yuv_data = vec![
            // Y plane (4x2)
            10, 20, 30, 40, 50, 60, 70, 80, // U plane (2x1)
            100, 110, // V plane (2x1)
            200, 210,
        ];
        let frame = VideoFrame {
            data: Bytes::from(yuv_data),
            pixel_format: PixelFormat::Yuv420p,
            width: 4,
            height: 2,
            timestamp: Duration::ZERO,
        };
        let nv12 = yuv420p_to_nv12(&frame).unwrap();
        assert_eq!(nv12.pixel_format, PixelFormat::Nv12);
        assert_eq!(nv12.data.len(), 12);
        // Y should be identical
        assert_eq!(&nv12.data[..8], &frame.data[..8]);
        // UV should be interleaved
        assert_eq!(nv12.data[8], 100);
        assert_eq!(nv12.data[9], 200);
        assert_eq!(nv12.data[10], 110);
        assert_eq!(nv12.data[11], 210);
    }

    #[test]
    fn yuv420p_to_nv12_passthrough() {
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 12]),
            pixel_format: PixelFormat::Nv12,
            width: 4,
            height: 2,
            timestamp: Duration::ZERO,
        };
        let result = yuv420p_to_nv12(&frame).unwrap();
        assert_eq!(result.data, frame.data);
    }

    #[test]
    fn convert_pixel_format_identity() {
        let frame = make_solid_yuv_frame(4, 4, 128);
        let result = convert_pixel_format(&frame, PixelFormat::Yuv420p).unwrap();
        assert_eq!(result.data, frame.data);
    }

    #[test]
    fn convert_pixel_format_yuv_to_rgb() {
        let frame = make_solid_yuv_frame(4, 4, 128);
        let result = convert_pixel_format(&frame, PixelFormat::Rgb24).unwrap();
        assert_eq!(result.pixel_format, PixelFormat::Rgb24);
    }

    #[test]
    fn convert_pixel_format_rgb_to_yuv() {
        let frame = make_solid_rgb_frame(4, 4, 128, 128, 128);
        let result = convert_pixel_format(&frame, PixelFormat::Yuv420p).unwrap();
        assert_eq!(result.pixel_format, PixelFormat::Yuv420p);
    }

    #[test]
    fn convert_pixel_format_rgb_to_nv12() {
        let frame = make_solid_rgb_frame(4, 4, 128, 128, 128);
        let result = convert_pixel_format(&frame, PixelFormat::Nv12).unwrap();
        assert_eq!(result.pixel_format, PixelFormat::Nv12);
    }

    #[test]
    fn convert_pixel_format_unsupported() {
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 100]),
            pixel_format: PixelFormat::Rgba32,
            width: 10,
            height: 10,
            timestamp: Duration::ZERO,
        };
        assert!(convert_pixel_format(&frame, PixelFormat::Rgb24).is_err());
    }

    #[test]
    fn convert_preserves_timestamp() {
        let frame = VideoFrame {
            data: Bytes::from(make_solid_yuv_frame(4, 4, 128).data.to_vec()),
            pixel_format: PixelFormat::Yuv420p,
            width: 4,
            height: 4,
            timestamp: Duration::from_millis(42),
        };
        let rgb = yuv420p_to_rgb24(&frame).unwrap();
        assert_eq!(rgb.timestamp, Duration::from_millis(42));
    }
}
