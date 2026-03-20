//! Scene detection in video
//!
//! Detects scene boundaries (cuts and transitions) by analyzing
//! inter-frame luminance histogram differences. Supports hard cuts
//! (chi-squared histogram distance) and gradual transitions
//! (rolling standard deviation of frame differences).
//!
//! ```rust,ignore
//! use tarang::ai::scene::{SceneDetector, SceneDetectionConfig};
//!
//! let mut detector = SceneDetector::new(SceneDetectionConfig::default());
//! // Feed frames from a decoder:
//! // detector.feed_frame(&frame, timestamp);
//! let boundaries = detector.finish();
//! ```

use crate::core::VideoFrame;
use std::collections::VecDeque;
use std::time::Duration;

/// Type of scene boundary detected.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneBoundaryType {
    HardCut,
    GradualTransition,
}

/// A detected scene boundary.
#[derive(Debug, Clone)]
pub struct SceneBoundary {
    pub timestamp: Duration,
    pub frame_index: u64,
    pub change_score: f64,
    pub boundary_type: SceneBoundaryType,
}

/// Configuration for scene detection.
#[derive(Debug, Clone)]
pub struct SceneDetectionConfig {
    pub hard_cut_threshold: f64,
    pub gradual_threshold: f64,
    pub min_scene_duration: Duration,
    pub histogram_bins: usize,
    pub rolling_window: usize,
}

impl Default for SceneDetectionConfig {
    fn default() -> Self {
        Self {
            hard_cut_threshold: 0.4,
            gradual_threshold: 0.15,
            min_scene_duration: Duration::from_secs(1),
            histogram_bins: 64,
            rolling_window: 10,
        }
    }
}

/// Stateful scene detector — feed frames one at a time.
pub struct SceneDetector {
    config: SceneDetectionConfig,
    prev_histogram: Option<Vec<f64>>,
    frame_scores: VecDeque<f64>,
    frame_index: u64,
    last_boundary_ts: Option<Duration>,
    boundaries: Vec<SceneBoundary>,
}

impl SceneDetector {
    pub fn new(mut config: SceneDetectionConfig) -> Self {
        if config.histogram_bins == 0 {
            config.histogram_bins = 64;
        }
        Self {
            config,
            prev_histogram: None,
            frame_scores: VecDeque::new(),
            frame_index: 0,
            last_boundary_ts: None,
            boundaries: Vec::new(),
        }
    }

    /// Feed a decoded video frame. Returns a boundary if one is detected.
    ///
    /// Returns `None` immediately if frame dimensions are zero.
    pub fn feed_frame(&mut self, frame: &VideoFrame) -> Option<SceneBoundary> {
        if frame.width == 0 || frame.height == 0 {
            return None;
        }
        let histogram = compute_luminance_histogram(frame, self.config.histogram_bins);
        self.frame_index += 1;

        let result = if let Some(prev) = &self.prev_histogram {
            let distance = chi_squared_distance(prev, &histogram);

            // Track rolling scores for gradual transition detection
            self.frame_scores.push_back(distance);
            if self.frame_scores.len() > self.config.rolling_window {
                self.frame_scores.pop_front();
            }

            // Check minimum scene duration
            let can_emit = match self.last_boundary_ts {
                None => true,
                Some(last) => {
                    frame.timestamp.saturating_sub(last) >= self.config.min_scene_duration
                }
            };

            if can_emit {
                if distance > self.config.hard_cut_threshold {
                    let boundary = SceneBoundary {
                        timestamp: frame.timestamp,
                        frame_index: self.frame_index,
                        change_score: distance,
                        boundary_type: SceneBoundaryType::HardCut,
                    };
                    self.last_boundary_ts = Some(frame.timestamp);
                    self.boundaries.push(boundary.clone());
                    Some(boundary)
                } else if self.frame_scores.len() >= 3 {
                    let std_dev = rolling_std_dev(&self.frame_scores);
                    if std_dev > self.config.gradual_threshold {
                        let boundary = SceneBoundary {
                            timestamp: frame.timestamp,
                            frame_index: self.frame_index,
                            change_score: std_dev,
                            boundary_type: SceneBoundaryType::GradualTransition,
                        };
                        self.last_boundary_ts = Some(frame.timestamp);
                        self.boundaries.push(boundary.clone());
                        Some(boundary)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        self.prev_histogram = Some(histogram);
        result
    }

    /// Return all detected boundaries.
    pub fn finish(self) -> Vec<SceneBoundary> {
        self.boundaries
    }
}

/// Convenience: detect all scenes in a frame iterator.
pub fn detect_scenes(
    frames: impl Iterator<Item = VideoFrame>,
    config: SceneDetectionConfig,
) -> Vec<SceneBoundary> {
    let mut detector = SceneDetector::new(config);
    for frame in frames {
        detector.feed_frame(&frame);
    }
    detector.finish()
}

/// Compute a normalized luminance histogram from a video frame.
pub fn compute_luminance_histogram(frame: &VideoFrame, bins: usize) -> Vec<f64> {
    let mut histogram = vec![0.0f64; bins];

    let luminance = super::video_utils::extract_luminance(frame);

    for &y in &luminance {
        let bin = (y as usize * bins) / 256;
        histogram[bin.min(bins - 1)] += 1.0;
    }

    // Normalize
    let total: f64 = histogram.iter().sum();
    if total > 0.0 {
        for bin in &mut histogram {
            *bin /= total;
        }
    }

    histogram
}

/// Chi-squared distance between two normalized histograms.
pub fn chi_squared_distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(ai, bi)| {
            let sum = ai + bi;
            if sum > 0.0 {
                (ai - bi).powi(2) / sum
            } else {
                0.0
            }
        })
        .sum::<f64>()
        * 0.5
}

fn rolling_std_dev(scores: &VecDeque<f64>) -> f64 {
    if scores.is_empty() {
        return 0.0;
    }
    let n = scores.len() as f64;
    let mean = scores.iter().sum::<f64>() / n;
    let variance = scores.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n;
    variance.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::PixelFormat;
    use bytes::Bytes;

    fn make_yuv_frame(width: u32, height: u32, y_value: u8, timestamp_ms: u64) -> VideoFrame {
        let y_size = (width * height) as usize;
        let chroma_size = width.div_ceil(2) as usize * height.div_ceil(2) as usize;
        let mut data = vec![y_value; y_size + 2 * chroma_size];
        // Set chroma to neutral
        for byte in &mut data[y_size..] {
            *byte = 128;
        }
        VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Yuv420p,
            width,
            height,
            timestamp: Duration::from_millis(timestamp_ms),
        }
    }

    #[test]
    fn identical_histograms_zero_distance() {
        let a = vec![0.25, 0.25, 0.25, 0.25];
        let b = vec![0.25, 0.25, 0.25, 0.25];
        assert!((chi_squared_distance(&a, &b)).abs() < 1e-10);
    }

    #[test]
    fn different_histograms_nonzero_distance() {
        let a = vec![1.0, 0.0, 0.0, 0.0];
        let b = vec![0.0, 0.0, 0.0, 1.0];
        assert!(chi_squared_distance(&a, &b) > 0.5);
    }

    #[test]
    fn histogram_from_uniform_frame() {
        let frame = make_yuv_frame(64, 64, 128, 0);
        let hist = compute_luminance_histogram(&frame, 64);
        // All pixels have Y=128, so one bin should be ~1.0
        let max_bin = hist.iter().cloned().fold(0.0f64, f64::max);
        assert!(max_bin > 0.9);
    }

    #[test]
    fn no_boundaries_for_identical_frames() {
        let config = SceneDetectionConfig::default();
        let mut detector = SceneDetector::new(config);
        for i in 0..30 {
            let frame = make_yuv_frame(64, 64, 128, i * 33);
            assert!(detector.feed_frame(&frame).is_none());
        }
        assert!(detector.finish().is_empty());
    }

    #[test]
    fn detects_hard_cut() {
        let config = SceneDetectionConfig {
            min_scene_duration: Duration::ZERO,
            ..Default::default()
        };
        let mut detector = SceneDetector::new(config);

        // 10 black frames
        for i in 0..10 {
            detector.feed_frame(&make_yuv_frame(64, 64, 0, i * 33));
        }

        // Then a white frame — hard cut
        let boundary = detector.feed_frame(&make_yuv_frame(64, 64, 255, 10 * 33));
        assert!(boundary.is_some());
        let b = boundary.unwrap();
        assert_eq!(b.boundary_type, SceneBoundaryType::HardCut);
        assert!(b.change_score > 0.3);
    }

    #[test]
    fn min_scene_duration_debounces() {
        let config = SceneDetectionConfig {
            min_scene_duration: Duration::from_millis(500),
            ..Default::default()
        };
        let mut detector = SceneDetector::new(config);

        // Black frame
        detector.feed_frame(&make_yuv_frame(64, 64, 0, 0));
        // White frame at 33ms — hard cut
        let b1 = detector.feed_frame(&make_yuv_frame(64, 64, 255, 33));
        assert!(b1.is_some());

        // Black again at 66ms — within min_scene_duration, should be suppressed
        let b2 = detector.feed_frame(&make_yuv_frame(64, 64, 0, 66));
        assert!(b2.is_none());
    }

    #[test]
    fn detect_scenes_convenience() {
        let frames: Vec<VideoFrame> = (0..10)
            .map(|i| make_yuv_frame(64, 64, 0, i * 33))
            .chain(std::iter::once(make_yuv_frame(64, 64, 255, 10 * 33)))
            .chain((11..20).map(|i| make_yuv_frame(64, 64, 255, i * 33)))
            .collect();

        let config = SceneDetectionConfig {
            min_scene_duration: Duration::ZERO,
            ..Default::default()
        };
        let boundaries = detect_scenes(frames.into_iter(), config);
        assert!(!boundaries.is_empty());
    }

    #[test]
    fn single_frame_no_boundary() {
        let config = SceneDetectionConfig::default();
        let mut detector = SceneDetector::new(config);
        assert!(
            detector
                .feed_frame(&make_yuv_frame(64, 64, 128, 0))
                .is_none()
        );
        assert!(detector.finish().is_empty());
    }

    #[test]
    fn rgb24_histogram() {
        let w = 64u32;
        let h = 64u32;
        let data = vec![128u8; (w * h * 3) as usize]; // gray
        let frame = VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Rgb24,
            width: w,
            height: h,
            timestamp: Duration::ZERO,
        };
        let hist = compute_luminance_histogram(&frame, 64);
        let total: f64 = hist.iter().sum();
        assert!((total - 1.0).abs() < 1e-6);
    }

    #[test]
    fn chi_squared_partial_overlap() {
        let a = vec![0.5, 0.5, 0.0, 0.0];
        let b = vec![0.0, 0.5, 0.5, 0.0];
        let d = chi_squared_distance(&a, &b);
        assert!(d > 0.0);
        assert!(d < 1.0);
    }

    #[test]
    fn chi_squared_empty_histograms() {
        let d = chi_squared_distance(&[], &[]);
        assert_eq!(d, 0.0);
    }

    #[test]
    fn chi_squared_all_zero_bins() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![0.0, 0.0, 0.0];
        assert_eq!(chi_squared_distance(&a, &b), 0.0);
    }

    #[test]
    fn chi_squared_single_bin() {
        let a = vec![1.0];
        let b = vec![0.0];
        let d = chi_squared_distance(&a, &b);
        assert!(d > 0.0);
    }

    #[test]
    fn rgba32_histogram_returns_empty() {
        let frame = VideoFrame {
            data: Bytes::from(vec![128u8; 16 * 16 * 4]),
            pixel_format: PixelFormat::Rgba32,
            width: 16,
            height: 16,
            timestamp: Duration::ZERO,
        };
        let hist = compute_luminance_histogram(&frame, 32);
        // RGBA32 should still produce a valid histogram via RGB path
        let total: f64 = hist.iter().sum();
        assert!((total - 1.0).abs() < 1e-6);
    }

    #[test]
    fn detect_scenes_empty_iterator() {
        let config = SceneDetectionConfig::default();
        let boundaries = detect_scenes(std::iter::empty(), config);
        assert!(boundaries.is_empty());
    }

    #[test]
    fn detect_scenes_single_frame() {
        let config = SceneDetectionConfig::default();
        let frames = vec![make_yuv_frame(32, 32, 128, 0)];
        let boundaries = detect_scenes(frames.into_iter(), config);
        assert!(boundaries.is_empty());
    }

    #[test]
    fn scene_detection_config_default_values() {
        let config = SceneDetectionConfig::default();
        assert_eq!(config.hard_cut_threshold, 0.4);
        assert_eq!(config.gradual_threshold, 0.15);
        assert_eq!(config.min_scene_duration, Duration::from_secs(1));
        assert_eq!(config.histogram_bins, 64);
        assert_eq!(config.rolling_window, 10);
    }

    #[test]
    fn yuv422p_histogram() {
        let w = 16u32;
        let h = 16u32;
        let y_size = (w * h) as usize;
        // YUV422p: Y plane + half-width U + half-width V
        let chroma_size = ((w / 2) * h) as usize;
        let mut data = vec![200u8; y_size];
        data.resize(y_size + 2 * chroma_size, 128);
        let frame = VideoFrame {
            data: Bytes::from(data),
            pixel_format: PixelFormat::Yuv422p,
            width: w,
            height: h,
            timestamp: Duration::ZERO,
        };
        let hist = compute_luminance_histogram(&frame, 32);
        let total: f64 = hist.iter().sum();
        assert!((total - 1.0).abs() < 1e-6);
    }

    #[test]
    fn multiple_hard_cuts_with_debounce() {
        let config = SceneDetectionConfig {
            min_scene_duration: Duration::from_millis(200),
            ..Default::default()
        };
        let mut detector = SceneDetector::new(config);
        let mut boundaries = Vec::new();

        // black frames 0-500ms
        for i in 0..15 {
            if let Some(b) = detector.feed_frame(&make_yuv_frame(32, 32, 0, i * 33)) {
                boundaries.push(b);
            }
        }
        // white frame at ~500ms
        if let Some(b) = detector.feed_frame(&make_yuv_frame(32, 32, 255, 500)) {
            boundaries.push(b);
        }
        // black frame at ~533ms (within debounce)
        if let Some(b) = detector.feed_frame(&make_yuv_frame(32, 32, 0, 533)) {
            boundaries.push(b);
        }
        // white frame at ~800ms (after debounce)
        if let Some(b) = detector.feed_frame(&make_yuv_frame(32, 32, 255, 800)) {
            boundaries.push(b);
        }

        // Should get at least the first hard cut
        assert!(!boundaries.is_empty());
        assert_eq!(boundaries[0].boundary_type, SceneBoundaryType::HardCut);
    }

    #[test]
    fn test_scene_zero_frame_dimensions() {
        let config = SceneDetectionConfig::default();
        let mut detector = SceneDetector::new(config);

        // 0x0 frame should return None without panicking
        let frame = VideoFrame {
            data: Bytes::from(vec![]),
            pixel_format: PixelFormat::Yuv420p,
            width: 0,
            height: 0,
            timestamp: Duration::from_millis(0),
        };
        assert!(detector.feed_frame(&frame).is_none());

        // 0-width frame
        let frame = VideoFrame {
            data: Bytes::from(vec![]),
            pixel_format: PixelFormat::Yuv420p,
            width: 0,
            height: 64,
            timestamp: Duration::from_millis(33),
        };
        assert!(detector.feed_frame(&frame).is_none());

        // 0-height frame
        let frame = VideoFrame {
            data: Bytes::from(vec![]),
            pixel_format: PixelFormat::Yuv420p,
            width: 64,
            height: 0,
            timestamp: Duration::from_millis(66),
        };
        assert!(detector.feed_frame(&frame).is_none());

        // Normal frame after zero frames should still work
        let frame = make_yuv_frame(32, 32, 128, 100);
        // No panic — first valid frame, no previous histogram
        assert!(detector.feed_frame(&frame).is_none());
    }
}
