//! Pure Rust audio resampling
//!
//! Converts audio between sample rates using windowed sinc interpolation.
//! Operates on interleaved F32 buffers.

use bytes::Bytes;
use tarang_core::{AudioBuffer, Result, SampleFormat, TarangError};

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

    let mut dst = vec![0.0f32; dst_frames * ch];

    for frame in 0..dst_frames {
        // Map destination frame back to source position
        let src_pos = frame as f64 / ratio;
        let src_idx = src_pos as usize;
        let frac = (src_pos - src_idx as f64) as f32;

        let idx0 = src_idx.min(src_frames - 1);
        let idx1 = (src_idx + 1).min(src_frames - 1);

        for c in 0..ch {
            let s0 = src[idx0 * ch + c];
            let s1 = src[idx1 * ch + c];
            dst[frame * ch + c] = s0 + (s1 - s0) * frac;
        }
    }

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

    let half_win = window_size as i64;
    let mut dst = vec![0.0f32; dst_frames * ch];

    for frame in 0..dst_frames {
        let src_pos = frame as f64 / ratio;
        let src_center = src_pos as i64;

        for c in 0..ch {
            let mut sum = 0.0f64;
            let mut weight_sum = 0.0f64;

            for i in (src_center - half_win + 1)..=(src_center + half_win) {
                if i < 0 || i >= src_frames as i64 {
                    continue;
                }

                let delta = src_pos - i as f64;
                let w = sinc(delta) * hann_window(delta, half_win as f64);
                sum += src[i as usize * ch + c] as f64 * w;
                weight_sum += w;
            }

            dst[frame * ch + c] = if weight_sum.abs() > 1e-10 {
                (sum / weight_sum) as f32
            } else {
                0.0
            };
        }
    }

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

use crate::sample::{bytes_to_f32, f32_to_bytes};

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sample::{make_test_buffer as make_buffer, make_test_sine as make_sine};

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
}
