//! Audio fingerprinting
//!
//! Chromaprint-style audio fingerprinting: decode audio to mono,
//! compute spectrograms via FFT, extract chroma features, and
//! hash into compact fingerprints for content identification.
//!
//! # Example
//!
//! ```rust,no_run
//! use tarang::ai::fingerprint::{compute_fingerprint, FingerprintConfig};
//!
//! let audio_buf = /* AudioBuffer from decoding */ panic!();
//! let config = FingerprintConfig::default();
//! let fp = compute_fingerprint(&audio_buf, &config).unwrap();
//! println!("fingerprint: {} hashes, {:.1}s", fp.hashes.len(), fp.duration_secs);
//! ```

use crate::core::{AudioBuffer, Result};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

/// A compact audio fingerprint for content identification.
#[derive(Debug, Clone)]
pub struct AudioFingerprint {
    pub hashes: Vec<u32>,
    pub duration_secs: f64,
}

/// Configuration for fingerprint computation.
#[derive(Debug, Clone)]
pub struct FingerprintConfig {
    pub sample_rate: u32,
    pub frame_size: usize,
    pub hop_size: usize,
    pub num_bands: usize,
}

impl Default for FingerprintConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            frame_size: 4096,
            hop_size: 2048,
            num_bands: 12,
        }
    }
}

/// Compute an audio fingerprint from an AudioBuffer.
///
/// The buffer should ideally be mono. If multi-channel, only the first
/// channel is used. The buffer is NOT resampled — caller should resample
/// to `config.sample_rate` beforehand for best results.
pub fn compute_fingerprint(
    buf: &AudioBuffer,
    config: &FingerprintConfig,
) -> Result<AudioFingerprint> {
    let samples = super::audio_utils::extract_mono_f32(buf)?;

    if samples.len() < config.frame_size {
        return Ok(AudioFingerprint {
            hashes: Vec::new(),
            duration_secs: samples.len() as f64 / buf.sample_rate as f64,
        });
    }

    let duration_secs = samples.len() as f64 / buf.sample_rate as f64;

    // Compute chroma features per frame
    let chroma_frames = compute_chroma_frames(&samples, config);

    // Hash consecutive chroma frame pairs (cap at 64K to prevent OOM on very long audio)
    const MAX_HASHES: usize = 65536;
    let frames_to_hash = if chroma_frames.len() > MAX_HASHES + 1 {
        &chroma_frames[..MAX_HASHES + 1]
    } else {
        &chroma_frames
    };
    let hashes = hash_chroma_frames(frames_to_hash);

    let fingerprint = AudioFingerprint {
        hashes,
        duration_secs,
    };

    tracing::debug!(
        hashes = fingerprint.hashes.len(),
        duration_secs = fingerprint.duration_secs,
        "fingerprint computed"
    );

    Ok(fingerprint)
}

/// Compare two fingerprints and return a similarity score (0.0..1.0).
pub fn fingerprint_match(a: &AudioFingerprint, b: &AudioFingerprint) -> f64 {
    if a.hashes.is_empty() || b.hashes.is_empty() {
        return 0.0;
    }

    // Sliding window comparison — find best alignment
    let (shorter, longer) = if a.hashes.len() <= b.hashes.len() {
        (&a.hashes, &b.hashes)
    } else {
        (&b.hashes, &a.hashes)
    };

    let mut best_score = 0.0;
    let max_offset = (longer.len() - shorter.len()).min(shorter.len());

    for offset in 0..=max_offset {
        let mut matching_bits = 0u64;
        let mut total_bits = 0u64;

        for (i, &hash_a) in shorter.iter().enumerate() {
            if offset + i < longer.len() {
                let hash_b = longer[offset + i];
                let xor = hash_a ^ hash_b;
                matching_bits += (32 - xor.count_ones()) as u64;
                total_bits += 32;
            }
        }

        if total_bits > 0 {
            let score = matching_bits as f64 / total_bits as f64;
            if score > best_score {
                best_score = score;
            }
        }
    }

    best_score
}

fn compute_chroma_frames(samples: &[f32], config: &FingerprintConfig) -> Vec<Vec<f64>> {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(config.frame_size);

    let hann_window: Vec<f32> = (0..config.frame_size)
        .map(|i| {
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / config.frame_size as f32).cos())
        })
        .collect();

    let half = config.frame_size / 2;
    let freq_per_bin = config.sample_rate as f64 / config.frame_size as f64;

    // Pre-compute frequency-to-band lookup table
    let band_lut: Vec<Option<usize>> = (0..half)
        .map(|i| {
            let freq = i as f64 * freq_per_bin;
            if i == 0 || !(60.0..=5000.0).contains(&freq) {
                None
            } else {
                let note = 12.0 * (freq / 440.0).log2();
                Some((note.rem_euclid(config.num_bands as f64)) as usize % config.num_bands)
            }
        })
        .collect();

    let mut frames = Vec::new();
    let mut fft_buf = vec![Complex::new(0.0f32, 0.0); config.frame_size];
    let mut chroma = vec![0.0f64; config.num_bands];
    let mut pos = 0;

    while pos + config.frame_size <= samples.len() {
        // Apply window in-place
        for (i, slot) in fft_buf.iter_mut().enumerate() {
            *slot = Complex::new(samples[pos + i] * hann_window[i], 0.0);
        }

        fft.process(&mut fft_buf);

        // Map magnitudes to chroma using LUT
        chroma.fill(0.0);
        for (i, slot) in fft_buf[..half].iter().enumerate() {
            if let Some(band) = band_lut[i] {
                let mag = (slot.re * slot.re + slot.im * slot.im).sqrt() as f64;
                chroma[band] += mag;
            }
        }

        // Normalize
        let max = chroma.iter().copied().fold(0.0f64, f64::max);
        if max > 0.0 {
            for c in &mut chroma {
                *c /= max;
            }
        }

        frames.push(chroma.clone());
        pos += config.hop_size;
    }

    frames
}

#[allow(dead_code)]
fn magnitudes_to_chroma(magnitudes: &[f64], config: &FingerprintConfig) -> Vec<f64> {
    let mut chroma = vec![0.0f64; config.num_bands];
    let freq_per_bin = config.sample_rate as f64 / config.frame_size as f64;

    for (i, &mag) in magnitudes.iter().enumerate().skip(1) {
        let freq = i as f64 * freq_per_bin;
        if !(60.0..=5000.0).contains(&freq) {
            continue;
        }
        // Map frequency to chroma band using log2
        let note = 12.0 * (freq / 440.0).log2();
        let band = ((note.rem_euclid(config.num_bands as f64)) as usize) % config.num_bands;
        chroma[band] += mag;
    }

    // Normalize
    let max = chroma.iter().cloned().fold(0.0f64, f64::max);
    if max > 0.0 {
        for c in &mut chroma {
            *c /= max;
        }
    }

    chroma
}

fn hash_chroma_frames(frames: &[Vec<f64>]) -> Vec<u32> {
    if frames.len() < 2 {
        return Vec::new();
    }

    let mut hashes = Vec::with_capacity(frames.len() - 1);

    for window in frames.windows(2) {
        let mut hash: u32 = 0;
        for (band, (curr, prev)) in window[1].iter().zip(window[0].iter()).enumerate() {
            if band >= 32 {
                break;
            }
            if curr > prev {
                hash |= 1 << band;
            }
        }
        hashes.push(hash);
    }

    hashes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SampleFormat;
    use bytes::Bytes;
    use std::time::Duration;

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

    fn make_noise_buffer(duration_secs: f32, sample_rate: u32, seed: u32) -> AudioBuffer {
        let num_frames = (sample_rate as f32 * duration_secs) as usize;
        let mut data = Vec::with_capacity(num_frames * 4);
        let mut state = seed;
        for _ in 0..num_frames {
            // Simple LCG pseudo-random
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            let sample = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
            data.extend_from_slice(&(sample * 0.3).to_le_bytes());
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

    #[test]
    fn deterministic_fingerprint() {
        let buf = make_sine_buffer(440.0, 2.0, 16000);
        let config = FingerprintConfig::default();
        let fp1 = compute_fingerprint(&buf, &config).unwrap();
        let fp2 = compute_fingerprint(&buf, &config).unwrap();
        assert_eq!(fp1.hashes, fp2.hashes);
    }

    #[test]
    fn identical_fingerprints_match_perfectly() {
        let buf = make_sine_buffer(440.0, 2.0, 16000);
        let config = FingerprintConfig::default();
        let fp = compute_fingerprint(&buf, &config).unwrap();
        let score = fingerprint_match(&fp, &fp);
        assert!((score - 1.0).abs() < 1e-10);
    }

    #[test]
    fn different_content_low_match() {
        let config = FingerprintConfig::default();
        let sine = compute_fingerprint(&make_sine_buffer(440.0, 2.0, 16000), &config).unwrap();
        let noise = compute_fingerprint(&make_noise_buffer(2.0, 16000, 42), &config).unwrap();
        let score = fingerprint_match(&sine, &noise);
        // Different content should score lower than identical (1.0)
        assert!(
            score < 0.95,
            "different content should not match nearly perfectly: {score}"
        );
    }

    #[test]
    fn same_frequency_different_amplitude() {
        let config = FingerprintConfig::default();
        let loud = make_sine_buffer(440.0, 2.0, 16000);
        // Quiet version: same frequency, lower amplitude
        let num_frames = 32000;
        let mut data = Vec::with_capacity(num_frames * 4);
        for i in 0..num_frames {
            let t = i as f32 / 16000.0;
            let sample = (t * 440.0 * std::f32::consts::TAU).sin() * 0.1;
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let quiet = AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::F32,
            channels: 1,
            sample_rate: 16000,
            num_frames,
            timestamp: Duration::ZERO,
        };

        let fp_loud = compute_fingerprint(&loud, &config).unwrap();
        let fp_quiet = compute_fingerprint(&quiet, &config).unwrap();
        let score = fingerprint_match(&fp_loud, &fp_quiet);
        // Same frequency content should match reasonably well
        assert!(score > 0.5, "same frequency should match well: {score}");
    }

    #[test]
    fn short_buffer_empty_fingerprint() {
        let data = vec![0u8; 100 * 4]; // 100 samples, less than frame_size
        let buf = AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::F32,
            channels: 1,
            sample_rate: 16000,
            num_frames: 100,
            timestamp: Duration::ZERO,
        };
        let config = FingerprintConfig::default();
        let fp = compute_fingerprint(&buf, &config).unwrap();
        assert!(fp.hashes.is_empty());
    }

    #[test]
    fn empty_fingerprints_zero_match() {
        let fp = AudioFingerprint {
            hashes: Vec::new(),
            duration_secs: 0.0,
        };
        assert_eq!(fingerprint_match(&fp, &fp), 0.0);
    }

    #[test]
    fn fingerprint_has_correct_duration() {
        let buf = make_sine_buffer(440.0, 3.0, 16000);
        let config = FingerprintConfig::default();
        let fp = compute_fingerprint(&buf, &config).unwrap();
        assert!((fp.duration_secs - 3.0).abs() < 0.01);
    }

    #[test]
    fn i16_input_works() {
        let num_frames = 8000;
        let mut data = Vec::with_capacity(num_frames * 2);
        for i in 0..num_frames {
            let t = i as f32 / 16000.0;
            let sample = ((t * 440.0 * std::f32::consts::TAU).sin() * 16000.0) as i16;
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let buf = AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::I16,
            channels: 1,
            sample_rate: 16000,
            num_frames,
            timestamp: Duration::ZERO,
        };
        let config = FingerprintConfig::default();
        let fp = compute_fingerprint(&buf, &config).unwrap();
        assert!(!fp.hashes.is_empty());
    }

    #[test]
    fn unsupported_format_returns_error() {
        let buf = AudioBuffer {
            data: Bytes::from(vec![0u8; 1000]),
            sample_format: SampleFormat::F64,
            channels: 1,
            sample_rate: 16000,
            num_frames: 125,
            timestamp: Duration::ZERO,
        };
        let config = FingerprintConfig::default();
        assert!(compute_fingerprint(&buf, &config).is_err());
    }

    #[test]
    fn one_empty_one_nonempty_zero_match() {
        let empty = AudioFingerprint {
            hashes: Vec::new(),
            duration_secs: 0.0,
        };
        let nonempty = AudioFingerprint {
            hashes: vec![0x12345678, 0xDEADBEEF],
            duration_secs: 1.0,
        };
        assert_eq!(fingerprint_match(&empty, &nonempty), 0.0);
        assert_eq!(fingerprint_match(&nonempty, &empty), 0.0);
    }

    #[test]
    fn stereo_f32_input() {
        let num_frames = 8000;
        let channels = 2u16;
        let mut data = Vec::with_capacity(num_frames * channels as usize * 4);
        for i in 0..num_frames {
            let t = i as f32 / 16000.0;
            let sample = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            // Write same sample to both channels
            data.extend_from_slice(&sample.to_le_bytes());
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let buf = AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::F32,
            channels,
            sample_rate: 16000,
            num_frames,
            timestamp: Duration::ZERO,
        };
        let config = FingerprintConfig::default();
        let fp = compute_fingerprint(&buf, &config).unwrap();
        // Should extract first channel only and still produce fingerprint
        assert!(!fp.hashes.is_empty());
    }

    #[test]
    fn fingerprint_config_default() {
        let config = FingerprintConfig::default();
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.frame_size, 4096);
        assert_eq!(config.hop_size, 2048);
        assert_eq!(config.num_bands, 12);
    }

    #[test]
    fn different_noise_seeds_different_fingerprints() {
        let config = FingerprintConfig::default();
        let fp1 = compute_fingerprint(&make_noise_buffer(2.0, 16000, 1), &config).unwrap();
        let fp2 = compute_fingerprint(&make_noise_buffer(2.0, 16000, 999), &config).unwrap();
        // Different seeds should produce different hashes
        assert_ne!(fp1.hashes, fp2.hashes);
    }

    #[test]
    fn stereo_i16_input() {
        let num_frames = 8000;
        let channels = 2u16;
        let mut data = Vec::with_capacity(num_frames * channels as usize * 2);
        for i in 0..num_frames {
            let t = i as f32 / 16000.0;
            let sample = ((t * 440.0 * std::f32::consts::TAU).sin() * 16000.0) as i16;
            data.extend_from_slice(&sample.to_le_bytes());
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let buf = AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::I16,
            channels,
            sample_rate: 16000,
            num_frames,
            timestamp: Duration::ZERO,
        };
        let config = FingerprintConfig::default();
        let fp = compute_fingerprint(&buf, &config).unwrap();
        assert!(!fp.hashes.is_empty());
    }

    #[test]
    fn test_fingerprint_empty_audio() {
        // Zero-length buffer should produce an empty fingerprint
        let buf = AudioBuffer {
            data: Bytes::new(),
            sample_format: SampleFormat::F32,
            channels: 1,
            sample_rate: 16000,
            num_frames: 0,
            timestamp: Duration::ZERO,
        };
        let config = FingerprintConfig::default();
        let fp = compute_fingerprint(&buf, &config).unwrap();
        assert!(fp.hashes.is_empty());
        assert_eq!(fp.duration_secs, 0.0);
    }
}
