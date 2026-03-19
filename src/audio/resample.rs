//! Pure Rust audio resampling
//!
//! Converts audio between sample rates using windowed sinc interpolation.
//! Operates on interleaved F32 buffers.

use crate::core::{AudioBuffer, Result, SampleFormat, TarangError};
use bytes::Bytes;

/// Resample an audio buffer to a target sample rate.
///
/// Uses linear interpolation — fast and sufficient for real-time playback.
/// For offline/high-quality use, sinc interpolation can be added later.
pub fn resample(buf: &AudioBuffer, target_rate: u32) -> Result<AudioBuffer> {
    if target_rate == 0 {
        return Err(TarangError::Pipeline("target sample rate is 0".to_string()));
    }
    if buf.sample_rate == 0 || buf.channels == 0 || buf.num_samples == 0 {
        return Err(TarangError::Pipeline("invalid source buffer".to_string()));
    }

    // No-op if rates match — return cheaply without cloning data
    if buf.sample_rate == target_rate {
        return Ok(AudioBuffer {
            data: buf.data.clone(), // Bytes::clone is O(1) ref-count bump
            sample_format: buf.sample_format,
            channels: buf.channels,
            sample_rate: buf.sample_rate,
            num_samples: buf.num_samples,
            timestamp: buf.timestamp,
        });
    }

    let src = bytes_to_f32(&buf.data);
    let ch = buf.channels as usize;
    let src_frames = buf.num_samples;

    let ratio = target_rate as f64 / buf.sample_rate as f64;
    let dst_frames = (src_frames as f64 * ratio).round() as usize;

    if dst_frames == 0 {
        return Err(TarangError::Pipeline("resampled to 0 frames".to_string()));
    }

    if dst_frames.checked_mul(ch).is_none() || dst_frames * ch * 4 >= 1_073_741_824 {
        return Err(TarangError::Pipeline(format!(
            "resampled output too large: {dst_frames} frames * {ch} channels exceeds 1GB cap"
        )));
    }

    let mut dst = vec![0.0f32; dst_frames * ch];

    // Pre-compute interpolation parameters into parallel arrays so the
    // compiler can auto-vectorize the inner interpolation loops.
    let mut idx0s = Vec::with_capacity(dst_frames);
    let mut idx1s = Vec::with_capacity(dst_frames);
    let mut fracs: Vec<f64> = Vec::with_capacity(dst_frames);

    for frame in 0..dst_frames {
        let src_pos = frame as f64 / ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;
        idx0s.push(src_idx.min(src_frames - 1));
        idx1s.push((src_idx + 1).min(src_frames - 1));
        fracs.push(frac);
    }

    if ch == 1 {
        // Mono fast path: process 4 frames at a time for auto-vectorization
        let chunks = dst_frames / 4;
        for chunk in 0..chunks {
            let base = chunk * 4;
            for k in 0..4 {
                let frame = base + k;
                let s0 = src[idx0s[frame]];
                dst[frame] = s0 + (src[idx1s[frame]] - s0) * fracs[frame] as f32;
            }
        }
        for frame in (chunks * 4)..dst_frames {
            let s0 = src[idx0s[frame]];
            dst[frame] = s0 + (src[idx1s[frame]] - s0) * fracs[frame] as f32;
        }
    } else if ch == 2 {
        // Stereo fast path: process both channels per frame together
        for frame in 0..dst_frames {
            let i0 = idx0s[frame] * 2;
            let i1 = idx1s[frame] * 2;
            let f = fracs[frame] as f32;
            let s0l = src[i0];
            let s0r = src[i0 + 1];
            dst[frame * 2] = s0l + (src[i1] - s0l) * f;
            dst[frame * 2 + 1] = s0r + (src[i1 + 1] - s0r) * f;
        }
    } else {
        // Generic fallback for ch > 2
        for frame in 0..dst_frames {
            let i0 = idx0s[frame];
            let i1 = idx1s[frame];
            let f = fracs[frame] as f32;
            for c in 0..ch {
                let s0 = src[i0 * ch + c];
                dst[frame * ch + c] = s0 + (src[i1 * ch + c] - s0) * f;
            }
        }
    }

    tracing::debug!(
        src_rate = buf.sample_rate,
        dst_rate = target_rate,
        src_frames = src_frames,
        dst_frames = dst_frames,
        channels = ch,
        "resample complete"
    );

    Ok(AudioBuffer {
        data: Bytes::copy_from_slice(f32_to_bytes(&dst)),
        sample_format: SampleFormat::F32,
        channels: buf.channels,
        sample_rate: target_rate,
        num_samples: dst_frames,
        timestamp: buf.timestamp,
    })
}

/// Resample with windowed sinc interpolation for higher quality.
/// `window_size` controls the number of sinc lobes (typically 8-64).
pub fn resample_sinc(
    buf: &AudioBuffer,
    target_rate: u32,
    window_size: usize,
) -> Result<AudioBuffer> {
    if target_rate == 0 {
        return Err(TarangError::Pipeline("target sample rate is 0".to_string()));
    }
    if buf.sample_rate == 0 || buf.channels == 0 || buf.num_samples == 0 {
        return Err(TarangError::Pipeline("invalid source buffer".to_string()));
    }
    if window_size == 0 || window_size > 1024 {
        return Err(TarangError::Pipeline(
            "window_size must be between 1 and 1024".to_string(),
        ));
    }
    if buf.sample_rate == target_rate {
        return Ok(AudioBuffer {
            data: buf.data.clone(),
            sample_format: buf.sample_format,
            channels: buf.channels,
            sample_rate: buf.sample_rate,
            num_samples: buf.num_samples,
            timestamp: buf.timestamp,
        });
    }

    let src = bytes_to_f32(&buf.data);
    let ch = buf.channels as usize;
    let src_frames = buf.num_samples;

    let ratio = target_rate as f64 / buf.sample_rate as f64;
    let dst_frames = (src_frames as f64 * ratio).round() as usize;

    if dst_frames == 0 {
        return Err(TarangError::Pipeline("resampled to 0 frames".to_string()));
    }

    if dst_frames.checked_mul(ch).is_none() || dst_frames * ch * 4 >= 1_073_741_824 {
        return Err(TarangError::Pipeline(format!(
            "resampled output too large: {dst_frames} frames * {ch} channels exceeds 1GB cap"
        )));
    }

    let half_win = window_size as i64;
    let tap_count = (2 * half_win) as usize; // taps per output frame
    let mut dst = vec![0.0f32; dst_frames * ch];

    // Pre-compute sinc * window weights for every (output_frame, tap) pair.
    // This moves all sin()/cos() calls out of the inner channel loop.
    let mut lut_weights: Vec<f64> = Vec::with_capacity(dst_frames * tap_count);
    let mut lut_src_indices: Vec<i64> = Vec::with_capacity(dst_frames * tap_count);
    let mut lut_weight_sums: Vec<f64> = Vec::with_capacity(dst_frames);

    for frame in 0..dst_frames {
        let src_pos = frame as f64 / ratio;
        let src_center = src_pos as i64;
        let mut weight_sum = 0.0f64;

        for tap in 0..tap_count {
            let i = src_center - half_win + 1 + tap as i64;
            lut_src_indices.push(i);
            if i < 0 || i >= src_frames as i64 {
                lut_weights.push(0.0);
            } else {
                let delta = src_pos - i as f64;
                let w = sinc(delta) * hann_window(delta, half_win as f64);
                lut_weights.push(w);
                weight_sum += w;
            }
        }
        lut_weight_sums.push(weight_sum);
    }

    for frame in 0..dst_frames {
        let base = frame * tap_count;
        let weight_sum = lut_weight_sums[frame];
        let norm = if weight_sum.abs() > 1e-10 {
            1.0 / weight_sum
        } else {
            0.0
        };

        for c in 0..ch {
            let mut sum = 0.0f64;

            for tap in 0..tap_count {
                let w = lut_weights[base + tap];
                if w == 0.0 {
                    continue;
                }
                let i = lut_src_indices[base + tap];
                if i < 0 || i >= src_frames as i64 {
                    continue;
                }
                sum += src[i as usize * ch + c] as f64 * w;
            }

            dst[frame * ch + c] = (sum * norm) as f32;
        }
    }

    tracing::debug!(
        src_rate = buf.sample_rate,
        dst_rate = target_rate,
        src_frames = src_frames,
        dst_frames = dst_frames,
        channels = ch,
        "resample complete"
    );

    Ok(AudioBuffer {
        data: Bytes::copy_from_slice(f32_to_bytes(&dst)),
        sample_format: SampleFormat::F32,
        channels: buf.channels,
        sample_rate: target_rate,
        num_samples: dst_frames,
        timestamp: buf.timestamp,
    })
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-10 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

fn hann_window(x: f64, half_width: f64) -> f64 {
    if x.abs() > half_width {
        0.0
    } else {
        0.5 * (1.0 + (std::f64::consts::PI * x / half_width).cos())
    }
}

use super::sample::{bytes_to_f32, f32_to_bytes};

#[cfg(test)]
mod tests {
    use super::*;

    use crate::audio::sample::{make_test_buffer as make_buffer, make_test_sine as make_sine};

    #[test]
    fn resample_noop() {
        let samples = make_sine(440.0, 44100, 1000, 2);
        let buf = make_buffer(&samples, 2, 44100);
        let out = resample(&buf, 44100).unwrap();
        assert_eq!(out.num_samples, 1000);
        assert_eq!(out.sample_rate, 44100);
    }

    #[test]
    fn resample_upsample() {
        let samples = make_sine(440.0, 44100, 4410, 1);
        let buf = make_buffer(&samples, 1, 44100);
        let out = resample(&buf, 48000).unwrap();

        assert_eq!(out.sample_rate, 48000);
        // 4410 * (48000/44100) ≈ 4800
        assert!((out.num_samples as i64 - 4800).abs() <= 1);
    }

    #[test]
    fn resample_downsample() {
        let samples = make_sine(440.0, 48000, 4800, 1);
        let buf = make_buffer(&samples, 1, 48000);
        let out = resample(&buf, 44100).unwrap();

        assert_eq!(out.sample_rate, 44100);
        assert!((out.num_samples as i64 - 4410).abs() <= 1);
    }

    #[test]
    fn resample_stereo() {
        let samples = make_sine(440.0, 44100, 4410, 2);
        let buf = make_buffer(&samples, 2, 44100);
        let out = resample(&buf, 48000).unwrap();

        assert_eq!(out.channels, 2);
        assert_eq!(out.sample_rate, 48000);
        // Data should have correct size: frames * channels * 4 bytes
        assert_eq!(out.data.len(), out.num_samples * 2 * 4);
    }

    #[test]
    fn resample_preserves_energy() {
        let samples = make_sine(440.0, 44100, 44100, 1); // 1 second
        let buf = make_buffer(&samples, 1, 44100);
        let out = resample(&buf, 48000).unwrap();

        let src_rms = rms(bytes_to_f32(&buf.data));
        let dst_rms = rms(bytes_to_f32(&out.data));

        // RMS should be roughly preserved (within 5%)
        assert!(
            (src_rms - dst_rms).abs() / src_rms < 0.05,
            "RMS diverged: src={src_rms}, dst={dst_rms}"
        );
    }

    #[test]
    fn resample_sinc_quality() {
        let samples = make_sine(440.0, 44100, 44100, 1);
        let buf = make_buffer(&samples, 1, 44100);
        let out = resample_sinc(&buf, 48000, 16).unwrap();

        assert_eq!(out.sample_rate, 48000);
        let src_rms = rms(bytes_to_f32(&buf.data));
        let dst_rms = rms(bytes_to_f32(&out.data));
        assert!(
            (src_rms - dst_rms).abs() / src_rms < 0.02,
            "sinc RMS diverged: src={src_rms}, dst={dst_rms}"
        );
    }

    #[test]
    fn resample_zero_rate_error() {
        let samples = make_sine(440.0, 44100, 100, 1);
        let buf = make_buffer(&samples, 1, 44100);
        assert!(resample(&buf, 0).is_err());
    }

    fn rms(samples: &[f32]) -> f32 {
        let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        (sum / samples.len() as f64).sqrt() as f32
    }

    #[test]
    fn resample_sinc_noop() {
        let samples = make_sine(440.0, 48000, 1000, 1);
        let buf = make_buffer(&samples, 1, 48000);
        let out = resample_sinc(&buf, 48000, 16).unwrap();
        assert_eq!(out.sample_rate, 48000);
        assert_eq!(out.data, buf.data);
    }

    #[test]
    fn resample_sinc_zero_rate_error() {
        let samples = make_sine(440.0, 44100, 100, 1);
        let buf = make_buffer(&samples, 1, 44100);
        assert!(resample_sinc(&buf, 0, 16).is_err());
    }

    #[test]
    fn resample_invalid_source() {
        let buf = AudioBuffer {
            data: Bytes::from(vec![]),
            sample_format: SampleFormat::F32,
            channels: 0,
            sample_rate: 0,
            num_samples: 0,
            timestamp: std::time::Duration::ZERO,
        };
        assert!(resample(&buf, 44100).is_err());
    }

    #[test]
    fn resample_large_ratio() {
        // 8000 → 48000 (6x upsample)
        let samples = make_sine(440.0, 8000, 800, 1);
        let buf = make_buffer(&samples, 1, 8000);
        let out = resample(&buf, 48000).unwrap();
        assert_eq!(out.sample_rate, 48000);
        assert!((out.num_samples as f64 / 4800.0 - 1.0).abs() < 0.01);
    }

    #[test]
    fn resample_preserves_channels() {
        let samples = make_sine(440.0, 44100, 1000, 2);
        let buf = make_buffer(&samples, 2, 44100);
        let out = resample(&buf, 22050).unwrap();
        assert_eq!(out.channels, 2);
    }

    #[test]
    fn resample_preserves_format() {
        let samples = make_sine(440.0, 44100, 1000, 1);
        let buf = make_buffer(&samples, 1, 44100);
        let out = resample(&buf, 48000).unwrap();
        assert_eq!(out.sample_format, SampleFormat::F32);
    }

    // ── accuracy tests ─────────────────────────────────────────────────

    #[test]
    fn test_resample_identity() {
        let samples = make_sine(440.0, 44100, 4410, 1);
        let buf = make_buffer(&samples, 1, 44100);
        let out = resample(&buf, 44100).unwrap();
        // Same-rate resample must be bit-exact (Bytes clone, no interpolation).
        assert_eq!(out.data, buf.data);
        assert_eq!(out.num_samples, buf.num_samples);
    }

    #[test]
    fn test_resample_downsample_upsample_roundtrip() {
        let n = 48000; // 1 second at 48 kHz
        let original = make_sine(440.0, 48000, n, 1);
        let buf = make_buffer(&original, 1, 48000);

        let down = resample(&buf, 16000).unwrap();
        let roundtrip = resample(&down, 48000).unwrap();

        let rt_samples = bytes_to_f32(&roundtrip.data);

        // Use the shorter length for comparison (may differ by ±1 frame).
        let len = original.len().min(rt_samples.len());

        // Pearson correlation — measures shape preservation.
        let mean_a: f64 = original[..len].iter().map(|s| *s as f64).sum::<f64>() / len as f64;
        let mean_b: f64 = rt_samples[..len].iter().map(|s| *s as f64).sum::<f64>() / len as f64;

        let mut cov = 0.0f64;
        let mut var_a = 0.0f64;
        let mut var_b = 0.0f64;
        for i in 0..len {
            let da = original[i] as f64 - mean_a;
            let db = rt_samples[i] as f64 - mean_b;
            cov += da * db;
            var_a += da * da;
            var_b += db * db;
        }
        let corr = cov / (var_a.sqrt() * var_b.sqrt());
        assert!(corr > 0.95, "roundtrip correlation too low: {corr:.4}");
    }

    #[test]
    fn test_resample_extreme_ratio_up() {
        // 8000 → 192000 (24x)
        let n = 800;
        let samples = make_sine(440.0, 8000, n, 1);
        let buf = make_buffer(&samples, 1, 8000);
        let out = resample(&buf, 192000).unwrap();

        // Frame count ≈ 24x
        let expected = (n as f64 * 24.0).round() as usize;
        assert!(
            (out.num_samples as i64 - expected as i64).abs() <= 1,
            "expected ~{expected} frames, got {}",
            out.num_samples
        );

        // No NaN / Inf
        let dst = bytes_to_f32(&out.data);
        for (i, &s) in dst.iter().enumerate() {
            assert!(s.is_finite(), "non-finite sample at index {i}: {s}");
        }

        // Energy approximately preserved (compare per-sample energy scaled by duration)
        let src_energy: f64 = samples.iter().map(|s| (*s as f64).powi(2)).sum();
        let dst_energy: f64 = dst.iter().map(|s| (*s as f64).powi(2)).sum();
        // Normalise to per-second energy (energy * rate / num_frames cancels to sum/frames * rate)
        let src_power = src_energy / n as f64;
        let dst_power = dst_energy / out.num_samples as f64;
        let rel = (src_power - dst_power).abs() / src_power;
        assert!(
            rel < 0.10,
            "energy diverged after 24x upsample: src_power={src_power:.4}, dst_power={dst_power:.4}, rel={rel:.4}"
        );
    }

    #[test]
    fn test_resample_extreme_ratio_down() {
        // 192000 → 8000 (24x downsample)
        let n = 19200; // 0.1 s at 192 kHz
        let samples = make_sine(440.0, 192000, n, 1);
        let buf = make_buffer(&samples, 1, 192000);
        let out = resample(&buf, 8000).unwrap();

        let expected = (n as f64 / 24.0).round() as usize;
        assert!(
            (out.num_samples as i64 - expected as i64).abs() <= 1,
            "expected ~{expected} frames, got {}",
            out.num_samples
        );

        let dst = bytes_to_f32(&out.data);
        for (i, &s) in dst.iter().enumerate() {
            assert!(s.is_finite(), "non-finite sample at index {i}: {s}");
        }

        let src_power: f64 = samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / n as f64;
        let dst_power: f64 =
            dst.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / out.num_samples as f64;
        let rel = (src_power - dst_power).abs() / src_power;
        assert!(
            rel < 0.10,
            "energy diverged after 24x downsample: src_power={src_power:.4}, dst_power={dst_power:.4}, rel={rel:.4}"
        );
    }

    #[test]
    fn test_resample_single_sample() {
        let samples = vec![0.5f32];
        let buf = make_buffer(&samples, 1, 8000);
        let out = resample(&buf, 48000).unwrap();
        // Should not panic and produce at least 1 sample.
        assert!(
            out.num_samples >= 1,
            "expected at least 1 output sample, got {}",
            out.num_samples
        );
        // Rough expectation: target_rate / source_rate = 6
        let expected = (48000.0_f64 / 8000.0_f64).round() as usize;
        assert!(
            (out.num_samples as i64 - expected as i64).abs() <= 1,
            "expected ~{expected} samples, got {}",
            out.num_samples
        );
    }

    #[test]
    fn test_resample_sinc_vs_linear_quality() {
        // Resample 440 Hz sine from 44100→48000 with both methods, then compare
        // each against an analytically generated reference at 48000 Hz.
        let n = 44100; // 1 second
        let src_samples = make_sine(440.0, 44100, n, 1);
        let buf = make_buffer(&src_samples, 1, 44100);

        let out_linear = resample(&buf, 48000).unwrap();
        let out_sinc = resample_sinc(&buf, 48000, 32).unwrap();

        // Analytically perfect 440 Hz at 48000 Hz
        let ref_samples = make_sine(440.0, 48000, out_linear.num_samples, 1);

        let lin_data = bytes_to_f32(&out_linear.data);
        let sinc_data = bytes_to_f32(&out_sinc.data);

        let snr_linear = compute_snr(&ref_samples, lin_data);
        let snr_sinc = compute_snr(&make_sine(440.0, 48000, out_sinc.num_samples, 1), sinc_data);

        assert!(
            snr_sinc > snr_linear,
            "sinc SNR ({snr_sinc:.1} dB) should exceed linear SNR ({snr_linear:.1} dB)"
        );
        // Sinc with 32-tap window should achieve decent SNR
        assert!(snr_sinc > 40.0, "sinc SNR too low: {snr_sinc:.1} dB");
    }

    #[test]
    fn test_resample_energy_conservation() {
        // Resample white noise 44100 → 48000 using sinc interpolation.
        // Compare RMS (root-mean-square) which is rate-independent.
        let n = 44100;
        let mut noise = Vec::with_capacity(n);
        // Deterministic pseudo-random (xorshift32)
        let mut state: u32 = 0xDEAD_BEEF;
        for _ in 0..n {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let s = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
            noise.push(s);
        }

        let buf = make_buffer(&noise, 1, 44100);
        let out = resample_sinc(&buf, 48000, 32).unwrap();

        let src_rms = rms(&noise);
        let dst_samples = bytes_to_f32(&out.data);
        let dst_rms = rms(dst_samples);

        let rel = (src_rms - dst_rms).abs() / src_rms;
        assert!(
            rel < 0.05,
            "energy not conserved: src_rms={src_rms:.4}, dst_rms={dst_rms:.4}, rel={rel:.4}"
        );
    }

    /// Compute SNR in dB between a reference signal and a test signal.
    fn compute_snr(reference: &[f32], test: &[f32]) -> f64 {
        let len = reference.len().min(test.len());
        let signal_power: f64 = reference[..len].iter().map(|s| (*s as f64).powi(2)).sum();
        let noise_power: f64 = reference[..len]
            .iter()
            .zip(test[..len].iter())
            .map(|(r, t)| ((*r as f64) - (*t as f64)).powi(2))
            .sum();
        if noise_power < 1e-20 {
            return 200.0; // effectively perfect
        }
        10.0 * (signal_power / noise_power).log10()
    }

    #[test]
    fn test_resample_extreme_ratio_rejected() {
        // Try an absurd upsample: 1 Hz -> 4_000_000_000 Hz with many samples.
        // The 1GB cap should reject this before allocating.
        let samples = make_sine(440.0, 8000, 100_000, 1);
        let buf = make_buffer(&samples, 1, 8000);
        // 100_000 frames * (4_000_000_000 / 8000) = 100_000 * 500_000 = 50 billion frames
        // That exceeds the 1GB cap.
        let result = resample(&buf, 4_000_000_000);
        assert!(
            result.is_err(),
            "absurd resample ratio should be rejected by 1GB cap"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("too large") || err_msg.contains("1GB"),
            "error should mention size limit, got: {err_msg}"
        );
    }
}
