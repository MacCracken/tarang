//! Speaker diarization — who-spoke-when segmentation.
//!
//! Detects speech segments and assigns speaker labels based on
//! spectral similarity. Uses energy-based VAD and MFCC-like features.
//!
//! ```rust,ignore
//! use tarang::ai::diarize::{diarize, DiarizeConfig};
//!
//! let config = DiarizeConfig::default();
//! let segments = diarize(&audio_buffer, &config).unwrap();
//! for seg in &segments {
//!     println!("Speaker {}: {:?}–{:?}", seg.speaker_id, seg.start, seg.end);
//! }
//! ```

use crate::core::{AudioBuffer, Result};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use std::time::Duration;

/// A speaker segment with timing and speaker label.
#[derive(Debug, Clone)]
pub struct SpeakerSegment {
    /// Start time of the segment.
    pub start: Duration,
    /// End time of the segment.
    pub end: Duration,
    /// 0-indexed speaker label.
    pub speaker_id: u32,
    /// Confidence score (0.0-1.0).
    pub confidence: f32,
}

/// Configuration for diarization.
#[derive(Debug, Clone)]
pub struct DiarizeConfig {
    /// Minimum duration for a speech segment to be kept.
    pub min_segment_duration: Duration,
    /// Energy threshold for voice activity detection.
    pub energy_threshold: f32,
    /// Maximum number of speakers to detect.
    pub max_speakers: u32,
}

impl Default for DiarizeConfig {
    fn default() -> Self {
        Self {
            min_segment_duration: Duration::from_millis(500),
            energy_threshold: 0.01,
            max_speakers: 10,
        }
    }
}

/// Frame size for VAD analysis (50ms at any sample rate).
const FRAME_DURATION_MS: u64 = 50;

/// Number of spectral centroid bands used as feature vector.
const NUM_FEATURE_BANDS: usize = 8;

/// Perform speaker diarization on an audio buffer.
///
/// Returns a list of speaker segments sorted by start time.
/// The buffer should contain speech audio — silence-only buffers
/// return an empty list.
pub fn diarize(buf: &AudioBuffer, config: &DiarizeConfig) -> Result<Vec<SpeakerSegment>> {
    let samples = super::audio_utils::extract_mono_f32(buf)?;
    let sample_rate = buf.sample_rate;

    if samples.is_empty() || sample_rate == 0 {
        return Ok(Vec::new());
    }

    let frame_size = (sample_rate as u64 * FRAME_DURATION_MS / 1000) as usize;
    if frame_size == 0 {
        return Ok(Vec::new());
    }

    // Step 1-2: Compute energy per frame and mark voiced/unvoiced
    let frame_energies = compute_frame_energies(&samples, frame_size);
    let voiced: Vec<bool> = frame_energies
        .iter()
        .map(|&e| e > config.energy_threshold)
        .collect();

    // Step 3: Group consecutive voiced frames into speech segments
    let raw_segments = group_voiced_frames(&voiced, frame_size, sample_rate);

    // Filter by minimum segment duration
    let segments: Vec<(Duration, Duration)> = raw_segments
        .into_iter()
        .filter(|(start, end)| end.saturating_sub(*start) >= config.min_segment_duration)
        .collect();

    if segments.is_empty() {
        return Ok(Vec::new());
    }

    // Step 4: Compute spectral feature vector for each segment
    let features: Vec<Vec<f64>> = segments
        .iter()
        .map(|(start, end)| {
            let start_sample = (start.as_secs_f64() * sample_rate as f64) as usize;
            let end_sample = ((end.as_secs_f64() * sample_rate as f64) as usize).min(samples.len());
            compute_spectral_features(&samples[start_sample..end_sample], sample_rate)
        })
        .collect();

    // Step 5: Cluster segments by feature similarity
    let max_speakers = (config.max_speakers as usize).min(segments.len());
    let labels = cluster_features(&features, max_speakers);

    // Step 6: Build output segments
    let result: Vec<SpeakerSegment> = segments
        .iter()
        .zip(labels.iter())
        .map(|((start, end), &speaker_id)| {
            // Confidence based on cluster cohesion — simplified to a constant
            // since we use nearest-neighbor clustering
            SpeakerSegment {
                start: *start,
                end: *end,
                speaker_id: speaker_id as u32,
                confidence: 0.8,
            }
        })
        .collect();

    tracing::debug!(
        segments = result.len(),
        speakers = result.iter().map(|s| s.speaker_id).max().unwrap_or(0) + 1,
        "diarization complete"
    );

    Ok(result)
}

/// Compute RMS energy for fixed-size non-overlapping frames.
fn compute_frame_energies(samples: &[f32], frame_size: usize) -> Vec<f32> {
    samples
        .chunks(frame_size)
        .map(|chunk| {
            let sum_sq: f32 = chunk.iter().map(|&s| s * s).sum();
            (sum_sq / chunk.len() as f32).sqrt()
        })
        .collect()
}

/// Group consecutive voiced frames into (start, end) Duration pairs.
fn group_voiced_frames(
    voiced: &[bool],
    frame_size: usize,
    sample_rate: u32,
) -> Vec<(Duration, Duration)> {
    let mut segments = Vec::new();
    let mut seg_start: Option<usize> = None;

    for (i, &is_voiced) in voiced.iter().enumerate() {
        match (is_voiced, seg_start) {
            (true, None) => seg_start = Some(i),
            (false, Some(start)) => {
                let start_time = frame_to_duration(start, frame_size, sample_rate);
                let end_time = frame_to_duration(i, frame_size, sample_rate);
                segments.push((start_time, end_time));
                seg_start = None;
            }
            _ => {}
        }
    }

    // Close any trailing segment
    if let Some(start) = seg_start {
        let start_time = frame_to_duration(start, frame_size, sample_rate);
        let end_time = frame_to_duration(voiced.len(), frame_size, sample_rate);
        segments.push((start_time, end_time));
    }

    segments
}

fn frame_to_duration(frame_idx: usize, frame_size: usize, sample_rate: u32) -> Duration {
    let samples = frame_idx * frame_size;
    Duration::from_secs_f64(samples as f64 / sample_rate as f64)
}

/// Compute spectral centroid features for a segment of audio.
///
/// Returns a feature vector of `NUM_FEATURE_BANDS` spectral band energies.
fn compute_spectral_features(samples: &[f32], sample_rate: u32) -> Vec<f64> {
    let fft_size = 1024;
    if samples.len() < fft_size {
        return vec![0.0; NUM_FEATURE_BANDS];
    }

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(fft_size);

    // Hann window
    let window: Vec<f32> = (0..fft_size)
        .map(|i| {
            0.5 * (1.0
                - (2.0 * std::f32::consts::PI * i as f32 / fft_size as f32).cos())
        })
        .collect();

    let half = fft_size / 2;
    let freq_per_bin = sample_rate as f64 / fft_size as f64;

    // Accumulate band energies across all frames in the segment
    let mut band_accum = vec![0.0f64; NUM_FEATURE_BANDS];
    let mut num_frames = 0usize;
    let mut fft_buf = vec![Complex::new(0.0f32, 0.0); fft_size];

    let hop = fft_size / 2;
    let mut pos = 0;
    while pos + fft_size <= samples.len() {
        for (i, slot) in fft_buf.iter_mut().enumerate() {
            *slot = Complex::new(samples[pos + i] * window[i], 0.0);
        }
        fft.process(&mut fft_buf);

        // Map bins to bands (log-spaced from ~100Hz to ~4000Hz)
        for i in 0..half {
            let freq = i as f64 * freq_per_bin;
            if !(100.0..=4000.0).contains(&freq) {
                continue;
            }
            // Log-scale band assignment
            let log_pos = ((freq / 100.0).ln()) / (4000.0 / 100.0_f64).ln();
            let band = (log_pos * NUM_FEATURE_BANDS as f64) as usize;
            let band = band.min(NUM_FEATURE_BANDS - 1);
            let mag = (fft_buf[i].re * fft_buf[i].re + fft_buf[i].im * fft_buf[i].im).sqrt()
                as f64;
            band_accum[band] += mag;
        }
        num_frames += 1;
        pos += hop;
    }

    // Normalize by frame count
    if num_frames > 0 {
        for v in &mut band_accum {
            *v /= num_frames as f64;
        }
    }

    // L2 normalize the feature vector
    let norm: f64 = band_accum.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm > 0.0 {
        for v in &mut band_accum {
            *v /= norm;
        }
    }

    band_accum
}

/// Simple nearest-neighbor clustering for feature vectors.
///
/// Assigns each segment to a cluster using a greedy approach:
/// the first segment starts cluster 0, and subsequent segments
/// join the nearest existing cluster if within a threshold,
/// otherwise start a new cluster (up to `max_speakers`).
fn cluster_features(features: &[Vec<f64>], max_speakers: usize) -> Vec<usize> {
    if features.is_empty() {
        return Vec::new();
    }

    // Threshold for "same speaker" — cosine distance
    const SAME_SPEAKER_THRESHOLD: f64 = 0.3;

    let mut centroids: Vec<Vec<f64>> = vec![features[0].clone()];
    let mut labels = vec![0usize; features.len()];

    for (i, feat) in features.iter().enumerate().skip(1) {
        // Find nearest centroid
        let mut best_dist = f64::MAX;
        let mut best_cluster = 0;

        for (c, centroid) in centroids.iter().enumerate() {
            let dist = cosine_distance(feat, centroid);
            if dist < best_dist {
                best_dist = dist;
                best_cluster = c;
            }
        }

        if best_dist < SAME_SPEAKER_THRESHOLD || centroids.len() >= max_speakers {
            labels[i] = best_cluster;
            // Update centroid with running average
            let n = labels[..=i].iter().filter(|&&l| l == best_cluster).count() as f64;
            for (j, v) in centroids[best_cluster].iter_mut().enumerate() {
                *v = *v * ((n - 1.0) / n) + feat[j] / n;
            }
        } else {
            // New speaker
            labels[i] = centroids.len();
            centroids.push(feat.clone());
        }
    }

    labels
}

/// Cosine distance between two vectors: 1 - cosine_similarity.
fn cosine_distance(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 1.0;
    }

    1.0 - (dot / (norm_a * norm_b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SampleFormat;
    use bytes::Bytes;

    fn make_sine_buffer(freq: f32, duration_secs: f32, sample_rate: u32) -> AudioBuffer {
        let num_frames = (sample_rate as f32 * duration_secs) as usize;
        let mut data = Vec::with_capacity(num_frames * 4);
        for i in 0..num_frames {
            let t = i as f32 / sample_rate as f32;
            let sample = (t * freq * std::f32::consts::TAU).sin() * 0.5;
            data.extend_from_slice(&sample.to_le_bytes());
        }
        AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::F32,
            channels: 1,
            sample_rate,
            num_frames,
            timestamp: Duration::ZERO,
        }
    }

    fn make_silence_buffer(duration_secs: f32, sample_rate: u32) -> AudioBuffer {
        let num_frames = (sample_rate as f32 * duration_secs) as usize;
        let data = vec![0u8; num_frames * 4]; // F32 zeros
        AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::F32,
            channels: 1,
            sample_rate,
            num_frames,
            timestamp: Duration::ZERO,
        }
    }

    #[test]
    fn silence_produces_no_segments() {
        let buf = make_silence_buffer(2.0, 16000);
        let config = DiarizeConfig::default();
        let segments = diarize(&buf, &config).unwrap();
        assert!(segments.is_empty(), "silence should produce no segments");
    }

    #[test]
    fn single_tone_produces_one_speaker() {
        let buf = make_sine_buffer(440.0, 2.0, 16000);
        let config = DiarizeConfig::default();
        let segments = diarize(&buf, &config).unwrap();
        assert!(!segments.is_empty(), "a tone should produce at least one segment");
        // All segments should be the same speaker
        let speaker_ids: Vec<u32> = segments.iter().map(|s| s.speaker_id).collect();
        assert!(
            speaker_ids.iter().all(|&id| id == speaker_ids[0]),
            "single tone should be one speaker, got: {speaker_ids:?}"
        );
    }

    #[test]
    fn config_defaults_are_sane() {
        let config = DiarizeConfig::default();
        assert_eq!(config.min_segment_duration, Duration::from_millis(500));
        assert_eq!(config.energy_threshold, 0.01);
        assert_eq!(config.max_speakers, 10);
    }

    #[test]
    fn segments_sorted_by_start_time() {
        let buf = make_sine_buffer(440.0, 3.0, 16000);
        let config = DiarizeConfig {
            min_segment_duration: Duration::from_millis(100),
            ..Default::default()
        };
        let segments = diarize(&buf, &config).unwrap();
        for window in segments.windows(2) {
            assert!(window[0].start <= window[1].start);
        }
    }

    #[test]
    fn confidence_in_valid_range() {
        let buf = make_sine_buffer(440.0, 2.0, 16000);
        let config = DiarizeConfig::default();
        let segments = diarize(&buf, &config).unwrap();
        for seg in &segments {
            assert!(
                (0.0..=1.0).contains(&seg.confidence),
                "confidence {} out of range",
                seg.confidence
            );
        }
    }

    #[test]
    fn empty_buffer_returns_empty() {
        let buf = AudioBuffer {
            data: Bytes::new(),
            sample_format: SampleFormat::F32,
            channels: 1,
            sample_rate: 16000,
            num_frames: 0,
            timestamp: Duration::ZERO,
        };
        let config = DiarizeConfig::default();
        let segments = diarize(&buf, &config).unwrap();
        assert!(segments.is_empty());
    }

    #[test]
    fn cosine_distance_identical_is_zero() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!(cosine_distance(&a, &b).abs() < 1e-10);
    }

    #[test]
    fn cosine_distance_orthogonal_is_one() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((cosine_distance(&a, &b) - 1.0).abs() < 1e-10);
    }
}
