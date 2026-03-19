//! Shared video frame utilities for luminance extraction
//!
//! Provides common luminance operations used by both scene detection
//! and thumbnail generation.

use tarang_core::{PixelFormat, VideoFrame};

/// BT.601 luminance coefficients.
const LUMA_R: f64 = 0.299;
const LUMA_G: f64 = 0.587;
const LUMA_B: f64 = 0.114;

/// Extract Y-plane luminance bytes from a video frame.
///
/// For YUV formats, returns the Y plane directly.
/// For RGB formats, computes BT.601 luminance per pixel.
/// Returns an empty Vec for unsupported formats.
pub fn extract_luminance(frame: &VideoFrame) -> Vec<u8> {
    match frame.pixel_format {
        PixelFormat::Yuv420p | PixelFormat::Yuv422p | PixelFormat::Yuv444p | PixelFormat::Nv12 => {
            let y_size = (frame.width * frame.height) as usize;
            frame.data[..y_size.min(frame.data.len())].to_vec()
        }
        PixelFormat::Rgb24 => {
            let pixels = frame.data.len() / 3;
            (0..pixels)
                .map(|i| {
                    let r = frame.data[i * 3] as f64;
                    let g = frame.data[i * 3 + 1] as f64;
                    let b = frame.data[i * 3 + 2] as f64;
                    (LUMA_R * r + LUMA_G * g + LUMA_B * b) as u8
                })
                .collect()
        }
        PixelFormat::Rgba32 => {
            let pixels = frame.data.len() / 4;
            (0..pixels)
                .map(|i| {
                    let r = frame.data[i * 4] as f64;
                    let g = frame.data[i * 4 + 1] as f64;
                    let b = frame.data[i * 4 + 2] as f64;
                    (LUMA_R * r + LUMA_G * g + LUMA_B * b) as u8
                })
                .collect()
        }
    }
}

/// Compute BT.601 luminance for a single RGB pixel.
#[inline]
pub fn rgb_to_luminance(r: f64, g: f64, b: f64) -> u8 {
    (LUMA_R * r + LUMA_G * g + LUMA_B * b) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::Duration;

    fn make_frame(format: PixelFormat, w: u32, h: u32, data: Vec<u8>) -> VideoFrame {
        VideoFrame {
            data: Bytes::from(data),
            pixel_format: format,
            width: w,
            height: h,
            timestamp: Duration::ZERO,
        }
    }

    #[test]
    fn yuv420p_luminance_is_y_plane() {
        let w = 4u32;
        let h = 2u32;
        let y_size = (w * h) as usize;
        let y_plane: Vec<u8> = (0..y_size as u8).collect();
        // Append some dummy UV data
        let mut data = y_plane.clone();
        data.extend_from_slice(&[128; 8]);

        let frame = make_frame(PixelFormat::Yuv420p, w, h, data);
        let luma = extract_luminance(&frame);
        assert_eq!(luma.len(), y_size);
        assert_eq!(luma, y_plane);
    }

    #[test]
    fn rgb24_bt601_conversion() {
        // Pure red pixel (255, 0, 0) -> 0.299*255 = 76.245 -> 76
        // Pure green pixel (0, 255, 0) -> 0.587*255 = 149.685 -> 149
        // Pure blue pixel (0, 0, 255) -> 0.114*255 = 29.07 -> 29
        let data = vec![255, 0, 0, 0, 255, 0, 0, 0, 255];
        let frame = make_frame(PixelFormat::Rgb24, 3, 1, data);
        let luma = extract_luminance(&frame);
        assert_eq!(luma.len(), 3);
        assert_eq!(luma[0], 76); // red
        assert_eq!(luma[1], 149); // green
        assert_eq!(luma[2], 29); // blue
    }

    #[test]
    fn rgb24_white_pixel() {
        // White (255, 255, 255) -> (0.299+0.587+0.114)*255 = 254.745 -> 254
        let data = vec![255, 255, 255];
        let frame = make_frame(PixelFormat::Rgb24, 1, 1, data);
        let luma = extract_luminance(&frame);
        assert_eq!(luma.len(), 1);
        // 0.299*255 + 0.587*255 + 0.114*255 = 255.0 (coefficients sum to 1.0)
        assert_eq!(luma[0], 255);
    }

    #[test]
    fn rgba32_computes_luminance() {
        // RGBA32 is supported and ignores the alpha channel
        let data = vec![255, 0, 0, 255, 0, 255, 0, 128];
        let frame = make_frame(PixelFormat::Rgba32, 2, 1, data);
        let luma = extract_luminance(&frame);
        assert_eq!(luma.len(), 2);
        assert_eq!(luma[0], 76); // red pixel
        assert_eq!(luma[1], 149); // green pixel
    }

    #[test]
    fn empty_frame_data() {
        let frame = make_frame(PixelFormat::Rgb24, 4, 4, vec![]);
        let luma = extract_luminance(&frame);
        assert!(luma.is_empty());
    }

    #[test]
    fn output_length_matches_width_times_height_yuv() {
        let w = 8u32;
        let h = 6u32;
        let y_size = (w * h) as usize;
        let mut data = vec![100u8; y_size];
        // Add chroma planes
        data.extend(std::iter::repeat(128u8).take(y_size / 2));

        let frame = make_frame(PixelFormat::Yuv420p, w, h, data);
        let luma = extract_luminance(&frame);
        assert_eq!(luma.len(), y_size);
    }

    #[test]
    fn output_length_matches_pixel_count_rgb() {
        let w = 3u32;
        let h = 2u32;
        let pixels = (w * h) as usize;
        let data = vec![128u8; pixels * 3];
        let frame = make_frame(PixelFormat::Rgb24, w, h, data);
        let luma = extract_luminance(&frame);
        assert_eq!(luma.len(), pixels);
    }
}
