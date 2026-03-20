//! Thumbnail generation at keyframes
//!
//! Selects representative video frames based on visual variance and
//! scene boundaries, then encodes them as JPEG or PNG thumbnails.
//!
//! ```rust,ignore
//! use tarang::ai::thumbnail::{generate_thumbnails, ThumbnailConfig};
//!
//! let config = ThumbnailConfig::default();
//! let thumbs = generate_thumbnails(&frames, &boundaries, &config).unwrap();
//! ```

use crate::core::{PixelFormat, Result, TarangError, VideoFrame};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ImageBuffer, ImageEncoder, Rgb, RgbImage};
use std::sync::Arc;
use std::time::Duration;

use super::scene::SceneBoundary;

/// Thumbnail output format.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailFormat {
    Jpeg,
    Png,
}

/// Strategy for scoring thumbnail candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailStrategy {
    /// Score by luminance variance (existing behavior).
    Variance,
    /// Score by content saliency (edge density + color diversity + skin tone).
    ContentBased,
}

/// Configuration for thumbnail generation.
#[derive(Debug, Clone)]
pub struct ThumbnailConfig {
    pub width: u32,
    pub height: u32,
    pub quality: u8,
    pub format: ThumbnailFormat,
    pub max_thumbnails: usize,
    pub min_variance: f64,
    pub strategy: ThumbnailStrategy,
}

impl Default for ThumbnailConfig {
    fn default() -> Self {
        Self {
            width: 320,
            height: 0, // auto from aspect ratio
            quality: 85,
            format: ThumbnailFormat::Jpeg,
            max_thumbnails: 5,
            min_variance: 500.0,
            strategy: ThumbnailStrategy::ContentBased,
        }
    }
}

/// A generated thumbnail.
#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub data: Vec<u8>,
    pub timestamp: Duration,
    pub width: u32,
    pub height: u32,
    pub format: ThumbnailFormat,
    pub score: f64,
}

/// Stateful thumbnail generator — feed candidate frames, then generate.
///
/// Uses `Arc<VideoFrame>` internally to avoid cloning megabyte-sized frame data.
pub struct ThumbnailGenerator {
    config: ThumbnailConfig,
    candidates: Vec<(Arc<VideoFrame>, f64)>,
}

impl ThumbnailGenerator {
    pub fn new(config: ThumbnailConfig) -> Self {
        Self {
            config,
            candidates: Vec::new(),
        }
    }

    /// Consider a frame as a thumbnail candidate.
    ///
    /// Wraps the frame in an `Arc` to avoid deep-cloning megabyte-sized pixel data.
    /// Only the top `max_thumbnails` candidates are retained.
    pub fn consider_frame(&mut self, frame: &VideoFrame, is_scene_boundary: bool) {
        let variance = luminance_variance(frame);
        if variance < self.config.min_variance {
            return; // reject low-variance frames (solid color, black, white)
        }

        let base_score = match self.config.strategy {
            ThumbnailStrategy::Variance => variance,
            ThumbnailStrategy::ContentBased => content_score(frame) as f64,
        };
        let score = base_score * if is_scene_boundary { 2.0 } else { 1.0 };

        // Arc::new clones the frame once; subsequent retentions are O(1) ref bumps
        self.candidates.push((Arc::new(frame.clone()), score));
        self.candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        self.candidates.truncate(self.config.max_thumbnails);
    }

    /// Generate thumbnails from the best candidates.
    pub fn generate(&self) -> Result<Vec<Thumbnail>> {
        let mut thumbnails = Vec::new();

        for (frame, score) in &self.candidates {
            let rgb = yuv420p_to_rgb24(frame)?;

            let (dst_w, dst_h) = self.compute_target_dims(frame.width, frame.height);

            let src_img: RgbImage = ImageBuffer::from_raw(frame.width, frame.height, rgb)
                .ok_or_else(|| TarangError::ImageError("failed to create image buffer".into()))?;

            let resized = image::imageops::resize(
                &src_img,
                dst_w,
                dst_h,
                image::imageops::FilterType::Triangle,
            );

            let encoded = encode_image(&resized, self.config.format, self.config.quality)?;

            thumbnails.push(Thumbnail {
                data: encoded,
                timestamp: frame.timestamp,
                width: dst_w,
                height: dst_h,
                format: self.config.format,
                score: *score,
            });
        }

        Ok(thumbnails)
    }

    fn compute_target_dims(&self, src_w: u32, src_h: u32) -> (u32, u32) {
        const MAX_DIM: u32 = 16384;

        // Default 0-valued config dimensions to source dimensions
        let cfg_w = if self.config.width == 0 {
            0
        } else {
            self.config.width.min(MAX_DIM)
        };
        let cfg_h = if self.config.height == 0 {
            0
        } else {
            self.config.height.min(MAX_DIM)
        };

        // Use source dims (capped) when src is 0
        let eff_src_w = if src_w == 0 { 1 } else { src_w };
        let eff_src_h = if src_h == 0 { 1 } else { src_h };

        if cfg_w == 0 && cfg_h == 0 {
            return (eff_src_w.min(MAX_DIM), eff_src_h.min(MAX_DIM));
        }
        if cfg_h == 0 {
            let aspect = eff_src_h as f64 / eff_src_w as f64;
            let h = (cfg_w as f64 * aspect).round() as u32;
            (cfg_w, h.clamp(1, MAX_DIM))
        } else if cfg_w == 0 {
            let aspect = eff_src_w as f64 / eff_src_h as f64;
            let w = (cfg_h as f64 * aspect).round() as u32;
            (w.clamp(1, MAX_DIM), cfg_h)
        } else {
            (cfg_w, cfg_h)
        }
    }
}

/// Convenience: generate thumbnails from frames with scene boundary info.
pub fn generate_thumbnails(
    frames: impl Iterator<Item = VideoFrame>,
    scene_boundaries: &[SceneBoundary],
    config: ThumbnailConfig,
) -> Result<Vec<Thumbnail>> {
    let boundary_indices: std::collections::HashSet<u64> =
        scene_boundaries.iter().map(|b| b.frame_index).collect();

    let mut generator = ThumbnailGenerator::new(config);
    for (i, frame) in frames.enumerate() {
        let is_boundary = boundary_indices.contains(&(i as u64 + 1));
        generator.consider_frame(&frame, is_boundary);
    }
    generator.generate()
}

/// Convert YUV420p frame data to RGB24.
pub fn yuv420p_to_rgb24(frame: &VideoFrame) -> Result<Vec<u8>> {
    if frame.pixel_format == PixelFormat::Rgb24 {
        return Ok(frame.data.to_vec());
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

    // Process two luma rows at a time to share chroma computations.
    // Pre-compute U/V contributions per chroma row using integer BT.601 math:
    //   R = Y + (359*V >> 8)
    //   G = Y - (88*U + 183*V >> 8)
    //   B = Y + (454*U >> 8)
    for chroma_row in 0..chroma_h {
        // Pre-compute chroma contributions for this row of chroma samples
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

        // Apply to the (up to) 2 luma rows that share this chroma row
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

    Ok(rgb)
}

/// Content-based frame scoring using saliency heuristics.
///
/// Combines multiple signals:
/// - Edge density (Sobel-like gradient magnitude)
/// - Color diversity (histogram spread across bins)
/// - Center-weighted interest (features near center score higher)
/// - Skin tone detection (proxy for face presence)
pub fn content_score(frame: &VideoFrame) -> f32 {
    let w = frame.width as usize;
    let h = frame.height as usize;
    if w < 3 || h < 3 {
        return 0.0;
    }

    let y_size = w * h;
    let luminance = super::video_utils::extract_luminance(frame);
    if luminance.len() < y_size {
        return 0.0;
    }

    // --- Edge density: average gradient magnitude ---
    let mut gradient_sum: u64 = 0;
    let mut gradient_count: u64 = 0;
    // Center weighting accumulator (weighted by gradient)
    let mut center_weighted_sum: f64 = 0.0;
    let mut center_weight_total: f64 = 0.0;

    let cx = w as f64 / 2.0;
    let cy = h as f64 / 2.0;
    let sigma_x = cx.max(1.0);
    let sigma_y = cy.max(1.0);

    for row in 1..h - 1 {
        for col in 1..w - 1 {
            let left = luminance[row * w + col - 1] as i32;
            let right = luminance[row * w + col + 1] as i32;
            let up = luminance[(row - 1) * w + col] as i32;
            let down = luminance[(row + 1) * w + col] as i32;

            let gx = (right - left).unsigned_abs();
            let gy = (down - up).unsigned_abs();
            let mag = gx + gy;

            gradient_sum += mag as u64;
            gradient_count += 1;

            // Gaussian-like center weight
            let dx = col as f64 - cx;
            let dy = row as f64 - cy;
            let weight =
                (-0.5 * (dx * dx / (sigma_x * sigma_x) + dy * dy / (sigma_y * sigma_y))).exp();
            center_weighted_sum += mag as f64 * weight;
            center_weight_total += weight;
        }
    }

    let edge_density = if gradient_count > 0 {
        gradient_sum as f32 / gradient_count as f32 / 255.0
    } else {
        0.0
    };

    let center_weight = if center_weight_total > 0.0 {
        (center_weighted_sum / center_weight_total / 255.0) as f32
    } else {
        0.0
    };

    // --- Color diversity: histogram spread across 16 bins ---
    let mut hist = [0u32; 16];
    for &y in &luminance[..y_size] {
        hist[(y >> 4) as usize] += 1;
    }
    let occupied = hist.iter().filter(|&&c| c > 0).count() as f32;
    let color_diversity = occupied / 16.0;

    // --- Skin tone detection (YCbCr UV range) ---
    let chroma_w = w.div_ceil(2);
    let chroma_h = h.div_ceil(2);
    let chroma_size = chroma_w * chroma_h;
    let skin_score = if frame.pixel_format == PixelFormat::Yuv420p
        && frame.data.len() >= y_size + 2 * chroma_size
    {
        let u_plane = &frame.data[y_size..y_size + chroma_size];
        let v_plane = &frame.data[y_size + chroma_size..];
        let mut skin_pixels = 0u64;
        for i in 0..chroma_size {
            let u = u_plane[i];
            let v = v_plane[i];
            if (100..=140).contains(&u) && (130..=170).contains(&v) {
                skin_pixels += 1;
            }
        }
        (skin_pixels as f32 / chroma_size as f32).min(1.0)
    } else {
        0.0
    };

    // Weighted combination
    0.4 * edge_density + 0.3 * color_diversity + 0.2 * skin_score + 0.1 * center_weight
}

/// Compute luminance variance for a video frame.
pub fn luminance_variance(frame: &VideoFrame) -> f64 {
    let luminance = super::video_utils::extract_luminance(frame);

    if luminance.is_empty() {
        return 0.0;
    }

    let n = luminance.len() as f64;
    let mean = luminance.iter().map(|&v| v as f64).sum::<f64>() / n;
    luminance
        .iter()
        .map(|&v| (v as f64 - mean).powi(2))
        .sum::<f64>()
        / n
}

fn encode_image(
    img: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    format: ThumbnailFormat,
    quality: u8,
) -> Result<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    let (w, h) = img.dimensions();
    let raw = img.as_raw();
    match format {
        ThumbnailFormat::Jpeg => {
            let encoder = JpegEncoder::new_with_quality(&mut buf, quality);
            encoder
                .write_image(raw, w, h, image::ExtendedColorType::Rgb8)
                .map_err(|e| TarangError::ImageError(format!("JPEG encode failed: {e}").into()))?;
        }
        ThumbnailFormat::Png => {
            let encoder = PngEncoder::new(&mut buf);
            encoder
                .write_image(raw, w, h, image::ExtendedColorType::Rgb8)
                .map_err(|e| TarangError::ImageError(format!("PNG encode failed: {e}").into()))?;
        }
    }
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn make_yuv_frame(width: u32, height: u32, pattern: u8, timestamp_ms: u64) -> VideoFrame {
        let y_size = (width * height) as usize;
        let chroma_w = width.div_ceil(2) as usize;
        let chroma_h = height.div_ceil(2) as usize;
        let mut data = Vec::with_capacity(y_size + 2 * chroma_w * chroma_h);

        // Y plane with pattern
        for i in 0..y_size {
            data.push(((i as u8).wrapping_mul(pattern)).wrapping_add(i as u8));
        }
        // U/V neutral
        data.resize(y_size + 2 * chroma_w * chroma_h, 128);

        VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Yuv420p,
            width,
            height,
            timestamp: Duration::from_millis(timestamp_ms),
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
    fn solid_frame_low_variance() {
        let frame = make_solid_yuv_frame(64, 64, 128);
        assert!(luminance_variance(&frame) < 1.0);
    }

    #[test]
    fn patterned_frame_high_variance() {
        let frame = make_yuv_frame(64, 64, 7, 0);
        assert!(luminance_variance(&frame) > 100.0);
    }

    #[test]
    fn yuv420p_to_rgb24_neutral_gray() {
        // Y=128, U=128, V=128 should produce approximately gray (128, 128, 128)
        let frame = make_solid_yuv_frame(4, 4, 128);
        let rgb = yuv420p_to_rgb24(&frame).unwrap();
        assert_eq!(rgb.len(), 4 * 4 * 3);
        // Check first pixel is approximately gray
        assert!((rgb[0] as i32 - 128).abs() < 3);
        assert!((rgb[1] as i32 - 128).abs() < 3);
        assert!((rgb[2] as i32 - 128).abs() < 3);
    }

    #[test]
    fn generator_rejects_solid_frames() {
        let config = ThumbnailConfig {
            min_variance: 500.0,
            ..Default::default()
        };
        let mut generator = ThumbnailGenerator::new(config);
        generator.consider_frame(&make_solid_yuv_frame(64, 64, 0), false);
        generator.consider_frame(&make_solid_yuv_frame(64, 64, 255), false);
        assert!(generator.candidates.is_empty());
    }

    #[test]
    fn generator_keeps_interesting_frames() {
        let config = ThumbnailConfig {
            min_variance: 10.0,
            max_thumbnails: 3,
            ..Default::default()
        };
        let mut generator = ThumbnailGenerator::new(config);
        for i in 0..10 {
            generator.consider_frame(&make_yuv_frame(64, 64, (i * 17 + 3) as u8, i * 33), false);
        }
        assert!(generator.candidates.len() <= 3);
    }

    #[test]
    fn generator_prefers_scene_boundaries() {
        let config = ThumbnailConfig {
            min_variance: 10.0,
            max_thumbnails: 1,
            ..Default::default()
        };
        let mut generator = ThumbnailGenerator::new(config);
        let regular = make_yuv_frame(64, 64, 7, 100);
        let boundary = make_yuv_frame(64, 64, 7, 200);
        generator.consider_frame(&regular, false);
        generator.consider_frame(&boundary, true);
        // Boundary frame has 2x score
        assert_eq!(generator.candidates.len(), 1);
        assert_eq!(
            generator.candidates[0].0.timestamp,
            Duration::from_millis(200)
        );
    }

    #[test]
    fn generate_jpeg_thumbnail() {
        let config = ThumbnailConfig {
            width: 32,
            height: 0,
            min_variance: 10.0,
            format: ThumbnailFormat::Jpeg,
            quality: 80,
            max_thumbnails: 1,
            ..Default::default()
        };
        let mut generator = ThumbnailGenerator::new(config);
        generator.consider_frame(&make_yuv_frame(64, 64, 7, 0), false);
        let thumbs = generator.generate().unwrap();
        assert_eq!(thumbs.len(), 1);
        // JPEG SOI marker
        assert_eq!(thumbs[0].data[0], 0xFF);
        assert_eq!(thumbs[0].data[1], 0xD8);
        assert_eq!(thumbs[0].width, 32);
    }

    #[test]
    fn generate_png_thumbnail() {
        let config = ThumbnailConfig {
            width: 32,
            height: 0,
            min_variance: 10.0,
            format: ThumbnailFormat::Png,
            max_thumbnails: 1,
            ..Default::default()
        };
        let mut generator = ThumbnailGenerator::new(config);
        generator.consider_frame(&make_yuv_frame(64, 64, 7, 0), false);
        let thumbs = generator.generate().unwrap();
        assert_eq!(thumbs.len(), 1);
        // PNG magic bytes
        assert_eq!(&thumbs[0].data[..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn aspect_ratio_preserved() {
        let config = ThumbnailConfig {
            width: 160,
            height: 0,
            min_variance: 0.0,
            ..Default::default()
        };
        let generator = ThumbnailGenerator::new(config);
        let (w, h) = generator.compute_target_dims(640, 480);
        assert_eq!(w, 160);
        assert_eq!(h, 120);
    }

    #[test]
    fn aspect_ratio_from_height_only() {
        let config = ThumbnailConfig {
            width: 0,
            height: 120,
            min_variance: 0.0,
            ..Default::default()
        };
        let generator = ThumbnailGenerator::new(config);
        let (w, h) = generator.compute_target_dims(640, 480);
        assert_eq!(h, 120);
        assert_eq!(w, 160);
    }

    #[test]
    fn target_dims_zero_zero_passthrough() {
        let config = ThumbnailConfig {
            width: 0,
            height: 0,
            min_variance: 0.0,
            ..Default::default()
        };
        let generator = ThumbnailGenerator::new(config);
        let (w, h) = generator.compute_target_dims(1920, 1080);
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);
    }

    #[test]
    fn target_dims_both_specified() {
        let config = ThumbnailConfig {
            width: 200,
            height: 100,
            min_variance: 0.0,
            ..Default::default()
        };
        let generator = ThumbnailGenerator::new(config);
        let (w, h) = generator.compute_target_dims(1920, 1080);
        assert_eq!(w, 200);
        assert_eq!(h, 100);
    }

    #[test]
    fn luminance_variance_rgb24() {
        // Varying RGB data should produce non-zero variance
        let w = 16u32;
        let h = 16u32;
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for i in 0..(w * h) as usize {
            let v = (i * 7) as u8;
            data.push(v);
            data.push(v);
            data.push(v);
        }
        let frame = VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Rgb24,
            width: w,
            height: h,
            timestamp: Duration::ZERO,
        };
        assert!(luminance_variance(&frame) > 100.0);
    }

    #[test]
    fn luminance_variance_rgba32_returns_zero() {
        // RGBA32 hits the catch-all branch returning 0.0
        let frame = VideoFrame {
            data: Bytes::from(vec![128u8; 16 * 16 * 4]),
            pixel_format: PixelFormat::Rgba32,
            width: 16,
            height: 16,
            timestamp: Duration::ZERO,
        };
        assert_eq!(luminance_variance(&frame), 0.0);
    }

    #[test]
    fn yuv420p_to_rgb24_rejects_unsupported_format() {
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
    fn yuv420p_to_rgb24_passthrough_rgb() {
        let data = vec![42u8; 4 * 4 * 3];
        let frame = VideoFrame {
            data: Bytes::from(data.clone()),
            pixel_format: PixelFormat::Rgb24,
            width: 4,
            height: 4,
            timestamp: Duration::ZERO,
        };
        let rgb = yuv420p_to_rgb24(&frame).unwrap();
        assert_eq!(rgb, data);
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
    fn generate_thumbnails_convenience() {
        let frames: Vec<VideoFrame> = (0..5)
            .map(|i| make_yuv_frame(32, 32, (i * 17 + 3) as u8, i * 100))
            .collect();
        let config = ThumbnailConfig {
            width: 16,
            height: 0,
            min_variance: 10.0,
            max_thumbnails: 2,
            format: ThumbnailFormat::Jpeg,
            quality: 75,
            ..Default::default()
        };
        let thumbs = generate_thumbnails(frames.into_iter(), &[], config).unwrap();
        assert!(thumbs.len() <= 2);
    }

    #[test]
    fn generate_thumbnails_empty_frames() {
        let config = ThumbnailConfig::default();
        let thumbs = generate_thumbnails(std::iter::empty(), &[], config).unwrap();
        assert!(thumbs.is_empty());
    }

    #[test]
    fn thumbnail_config_default() {
        let config = ThumbnailConfig::default();
        assert_eq!(config.width, 320);
        assert_eq!(config.height, 0);
        assert_eq!(config.quality, 85);
        assert_eq!(config.format, ThumbnailFormat::Jpeg);
        assert_eq!(config.max_thumbnails, 5);
        assert_eq!(config.min_variance, 500.0);
        assert_eq!(config.strategy, ThumbnailStrategy::ContentBased);
    }

    #[test]
    fn test_thumbnail_zero_dimensions_default() {
        // Config 0x0 with source 0x0 should default to 1x1 (minimum valid)
        let config = ThumbnailConfig {
            width: 0,
            height: 0,
            min_variance: 0.0,
            ..Default::default()
        };
        let generator = ThumbnailGenerator::new(config);
        let (w, h) = generator.compute_target_dims(0, 0);
        assert_eq!(w, 1);
        assert_eq!(h, 1);

        // Config 0x0 with valid source should pass through source dims
        let (w, h) = generator.compute_target_dims(1920, 1080);
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);
    }

    #[test]
    fn test_thumbnail_huge_dimensions_capped() {
        // Config dimensions > 16384 should be capped
        let config = ThumbnailConfig {
            width: 20000,
            height: 20000,
            min_variance: 0.0,
            ..Default::default()
        };
        let generator = ThumbnailGenerator::new(config);
        let (w, h) = generator.compute_target_dims(1920, 1080);
        assert_eq!(w, 16384);
        assert_eq!(h, 16384);

        // Only width huge, height auto
        let config = ThumbnailConfig {
            width: 20000,
            height: 0,
            min_variance: 0.0,
            ..Default::default()
        };
        let generator = ThumbnailGenerator::new(config);
        let (w, h) = generator.compute_target_dims(1920, 1080);
        assert_eq!(w, 16384);
        assert!(h <= 16384);
        assert!(h > 0);

        // Source dimensions huge with 0x0 config
        let config = ThumbnailConfig {
            width: 0,
            height: 0,
            min_variance: 0.0,
            ..Default::default()
        };
        let generator = ThumbnailGenerator::new(config);
        let (w, h) = generator.compute_target_dims(20000, 20000);
        assert_eq!(w, 16384);
        assert_eq!(h, 16384);
    }

    #[test]
    fn content_score_solid_black_is_low() {
        let frame = make_solid_yuv_frame(64, 64, 0);
        let score = content_score(&frame);
        assert!(
            score < 0.05,
            "solid black score should be very low, got {score}"
        );
    }

    #[test]
    fn content_score_edges_higher_than_flat() {
        let flat = make_solid_yuv_frame(64, 64, 128);
        let edgy = make_yuv_frame(64, 64, 7, 0);
        let flat_score = content_score(&flat);
        let edgy_score = content_score(&edgy);
        assert!(
            edgy_score > flat_score,
            "edgy frame ({edgy_score}) should score higher than flat ({flat_score})"
        );
    }

    #[test]
    fn strategy_selection_variance_vs_content() {
        let frame = make_yuv_frame(64, 64, 7, 100);

        // Variance strategy
        let config_var = ThumbnailConfig {
            min_variance: 0.0,
            max_thumbnails: 1,
            strategy: ThumbnailStrategy::Variance,
            ..Default::default()
        };
        let mut gen_var = ThumbnailGenerator::new(config_var);
        gen_var.consider_frame(&frame, false);
        let var_score = gen_var.candidates[0].1;

        // ContentBased strategy
        let config_cb = ThumbnailConfig {
            min_variance: 0.0,
            max_thumbnails: 1,
            strategy: ThumbnailStrategy::ContentBased,
            ..Default::default()
        };
        let mut gen_cb = ThumbnailGenerator::new(config_cb);
        gen_cb.consider_frame(&frame, false);
        let cb_score = gen_cb.candidates[0].1;

        // Variance score is raw variance (typically hundreds+), content score is 0..1 range
        assert!(
            var_score > 1.0,
            "variance score should be large, got {var_score}"
        );
        assert!(
            cb_score < 2.0,
            "content score should be in small range, got {cb_score}"
        );
        assert!(
            (var_score - cb_score).abs() > 0.01,
            "scores should differ between strategies"
        );
    }
}
