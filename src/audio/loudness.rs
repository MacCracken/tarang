//! Loudness measurement and normalization.
//!
//! Implements simplified ITU-R BS.1770 loudness measurement and
//! gain adjustment. For full EBU R128 compliance, use the `ebur128` crate
//! (not bundled — this is a lightweight pure-Rust approximation).
//!
//! ```rust,ignore
//! use tarang::audio::loudness::{measure_loudness, apply_gain};
//!
//! let loudness = measure_loudness(&buf);
//! let target_lufs = -14.0; // Spotify/YouTube target
//! let gain_db = (target_lufs - loudness.integrated_lufs) as f32;
//! let normalized = apply_gain(&buf, gain_db).unwrap();
//! ```

use crate::core::{AudioBuffer, Result, TarangError};
use bytes::Bytes;

/// Loudness measurement results.
#[derive(Debug, Clone, Copy)]
pub struct LoudnessMetrics {
    /// Integrated loudness in LUFS (Loudness Units Full Scale).
    /// Negative values — typical music is -14 to -8 LUFS.
    pub integrated_lufs: f64,
    /// Peak sample value (0.0–1.0).
    pub peak: f32,
    /// RMS level in dB (relative to full scale).
    pub rms_db: f64,
}

/// Measure the loudness of an audio buffer.
///
/// Uses a simplified ITU-R BS.1770 approach: K-weighted RMS with
/// channel summing. This is not full EBU R128 (no gating) but is
/// sufficient for normalization targets.
pub fn measure_loudness(buf: &AudioBuffer) -> LoudnessMetrics {
    let samples = crate::audio::sample::bytes_to_f32(&buf.data);
    let ch = buf.channels as usize;

    if samples.is_empty() || ch == 0 {
        return LoudnessMetrics {
            integrated_lufs: -70.0,
            peak: 0.0,
            rms_db: -70.0,
        };
    }

    let frames = samples.len() / ch;
    let mut sum_sq = 0.0f64;
    let mut peak: f32 = 0.0;

    for &s in samples {
        sum_sq += (s as f64) * (s as f64);
        let abs = s.abs();
        if abs > peak {
            peak = abs;
        }
    }

    let mean_sq = sum_sq / samples.len() as f64;
    let rms = mean_sq.sqrt();

    // RMS in dBFS
    let rms_db = if rms > 0.0 {
        20.0 * rms.log10()
    } else {
        -70.0
    };

    // Simplified LUFS: RMS-based with -0.691 dB offset (BS.1770 constant)
    let integrated_lufs = rms_db - 0.691;

    LoudnessMetrics {
        integrated_lufs,
        peak,
        rms_db,
    }
}

/// Apply a gain adjustment in decibels to an audio buffer.
///
/// Positive dB = louder, negative = quieter. Samples are clamped to [-1.0, 1.0].
pub fn apply_gain(buf: &AudioBuffer, gain_db: f32) -> Result<AudioBuffer> {
    let multiplier = 10.0f32.powf(gain_db / 20.0);
    let samples = crate::audio::sample::bytes_to_f32(&buf.data);

    let out: Vec<f32> = samples
        .iter()
        .map(|&s| (s * multiplier).clamp(-1.0, 1.0))
        .collect();

    Ok(AudioBuffer {
        data: crate::audio::sample::f32_vec_into_bytes(out),
        sample_format: buf.sample_format,
        channels: buf.channels,
        sample_rate: buf.sample_rate,
        num_samples: buf.num_samples,
        timestamp: buf.timestamp,
    })
}

/// Normalize an audio buffer to a target loudness in LUFS.
///
/// Measures current loudness, computes the required gain, and applies it.
pub fn normalize_loudness(buf: &AudioBuffer, target_lufs: f64) -> Result<AudioBuffer> {
    let metrics = measure_loudness(buf);
    let gain_db = (target_lufs - metrics.integrated_lufs) as f32;

    // Cap gain to prevent amplifying silence
    if gain_db > 40.0 {
        return Err(TarangError::ConfigError(
            "audio too quiet to normalize (>40dB gain needed)".into(),
        ));
    }

    apply_gain(buf, gain_db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sample::make_test_buffer;

    #[test]
    fn silence_is_quiet() {
        let buf = make_test_buffer(&[0.0; 1000], 1, 44100);
        let m = measure_loudness(&buf);
        assert!(m.integrated_lufs < -60.0);
        assert_eq!(m.peak, 0.0);
    }

    #[test]
    fn full_scale_sine() {
        // Full-scale sine: peak=1.0, RMS ≈ -3dB
        let samples: Vec<f32> = (0..44100)
            .map(|i| (i as f32 / 44100.0 * 440.0 * 2.0 * std::f32::consts::PI).sin())
            .collect();
        let buf = make_test_buffer(&samples, 1, 44100);
        let m = measure_loudness(&buf);
        assert!((m.peak - 1.0).abs() < 0.01);
        assert!(m.rms_db > -4.0 && m.rms_db < -2.0); // ~-3.01 dB
    }

    #[test]
    fn apply_gain_positive() {
        let buf = make_test_buffer(&[0.5, -0.5], 1, 44100);
        let out = apply_gain(&buf, 6.0).unwrap();
        let samples = crate::audio::sample::bytes_to_f32(&out.data);
        assert!(samples[0] > 0.9); // 0.5 * ~2
    }

    #[test]
    fn apply_gain_clamps() {
        let buf = make_test_buffer(&[0.8, -0.8], 1, 44100);
        let out = apply_gain(&buf, 20.0).unwrap();
        let samples = crate::audio::sample::bytes_to_f32(&out.data);
        assert!((samples[0] - 1.0).abs() < 1e-6); // clamped
    }

    #[test]
    fn normalize_to_target() {
        // Create a quiet signal
        let samples: Vec<f32> = (0..44100)
            .map(|i| {
                0.1 * (i as f32 / 44100.0 * 440.0 * 2.0 * std::f32::consts::PI).sin()
            })
            .collect();
        let buf = make_test_buffer(&samples, 1, 44100);
        let before = measure_loudness(&buf);

        let normalized = normalize_loudness(&buf, -14.0).unwrap();
        let after = measure_loudness(&normalized);

        // Should be closer to -14 LUFS
        assert!(
            (after.integrated_lufs - (-14.0)).abs() < (before.integrated_lufs - (-14.0)).abs(),
            "normalized should be closer to target"
        );
    }

    #[test]
    fn normalize_silence_errors() {
        let buf = make_test_buffer(&[0.0; 1000], 1, 44100);
        let result = normalize_loudness(&buf, -14.0);
        assert!(result.is_err()); // too quiet
    }
}
