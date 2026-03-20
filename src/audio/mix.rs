//! Channel mixing — stereo↔mono, 5.1 downmix
//!
//! Operates on interleaved F32 audio buffers.

use crate::core::{AudioBuffer, Result, SampleFormat, TarangError};
use bytes::Bytes;

/// Target channel layout for mixing
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLayout {
    Mono,
    Stereo,
}

/// Mix an audio buffer to the target channel layout.
pub fn mix_channels(buf: &AudioBuffer, target: ChannelLayout) -> Result<AudioBuffer> {
    let src_ch = buf.channels as usize;
    let target_ch = match target {
        ChannelLayout::Mono => 1,
        ChannelLayout::Stereo => 2,
    };

    if src_ch == target_ch {
        return Ok(AudioBuffer {
            data: buf.data.clone(), // Bytes::clone is O(1) ref-count bump
            sample_format: buf.sample_format,
            channels: buf.channels,
            sample_rate: buf.sample_rate,
            num_samples: buf.num_samples,
            timestamp: buf.timestamp,
        });
    }
    if src_ch == 0 || buf.num_samples == 0 {
        return Err(TarangError::ConfigError("invalid source buffer".into()));
    }

    let src = bytes_to_f32(&buf.data);
    // Derive frame count from actual data length to handle callers that
    // set num_samples to total interleaved samples rather than frames.
    let frames = src.len() / src_ch.max(1);
    if frames == 0 {
        return Err(TarangError::ConfigError("source buffer has no frames".into()));
    }
    let required_dst = frames
        .checked_mul(target_ch)
        .ok_or_else(|| TarangError::ConfigError("destination size overflow".into()))?;
    if required_dst.checked_mul(4).is_none() {
        return Err(TarangError::ConfigError(
            "destination buffer size exceeds addressable memory".into(),
        ));
    }
    let mut dst = vec![0.0f32; required_dst];

    match (src_ch, target_ch) {
        // Stereo → Mono: average L+R
        (2, 1) => {
            for i in 0..frames {
                dst[i] = (src[i * 2] + src[i * 2 + 1]) * 0.5;
            }
        }
        // Mono → Stereo: duplicate
        (1, 2) => {
            for i in 0..frames {
                dst[i * 2] = src[i];
                dst[i * 2 + 1] = src[i];
            }
        }
        // 5.1 (6ch) → Stereo: ITU-R BS.775 downmix
        // L' = L + 0.707*C + 0.707*Ls
        // R' = R + 0.707*C + 0.707*Rs
        (6, 2) => {
            let k = std::f32::consts::FRAC_1_SQRT_2; // 0.707
            for i in 0..frames {
                let fl = src[i * 6]; // Front Left
                let fr = src[i * 6 + 1]; // Front Right
                let fc = src[i * 6 + 2]; // Front Center
                let _lfe = src[i * 6 + 3]; // LFE (discarded in standard downmix)
                let sl = src[i * 6 + 4]; // Surround Left
                let sr = src[i * 6 + 5]; // Surround Right

                dst[i * 2] = fl + k * fc + k * sl;
                dst[i * 2 + 1] = fr + k * fc + k * sr;
            }
        }
        // 5.1 (6ch) → Mono: downmix to stereo first, then average
        (6, 1) => {
            let k = std::f32::consts::FRAC_1_SQRT_2;
            for i in 0..frames {
                let fl = src[i * 6];
                let fr = src[i * 6 + 1];
                let fc = src[i * 6 + 2];
                let _lfe = src[i * 6 + 3];
                let sl = src[i * 6 + 4];
                let sr = src[i * 6 + 5];

                let l = fl + k * fc + k * sl;
                let r = fr + k * fc + k * sr;
                dst[i] = (l + r) * 0.5;
            }
        }
        // Generic N→Mono: average all channels
        (n, 1) => {
            let inv = 1.0 / n as f32;
            for i in 0..frames {
                let mut sum = 0.0f32;
                for c in 0..n {
                    sum += src[i * n + c];
                }
                dst[i] = sum * inv;
            }
        }
        // Generic N→Stereo: map first two channels, mix remaining equally into both
        (n, 2) => {
            for i in 0..frames {
                let mut l = src[i * n];
                let mut r = if n > 1 { src[i * n + 1] } else { src[i * n] };

                // Mix any remaining channels equally into L and R
                if n > 2 {
                    let extra_gain = 0.5 / (n - 2) as f32;
                    for c in 2..n {
                        let s = src[i * n + c] * extra_gain;
                        l += s;
                        r += s;
                    }
                }

                dst[i * 2] = l;
                dst[i * 2 + 1] = r;
            }
        }
        _ => {
            return Err(TarangError::ConfigError(
                format!("unsupported channel mix: {src_ch} → {target_ch}").into(),
            ));
        }
    }

    tracing::debug!(
        src_channels = src_ch,
        dst_channels = target_ch,
        frames = frames,
        "mix complete"
    );

    Ok(AudioBuffer {
        data: Bytes::copy_from_slice(f32_to_bytes(&dst)),
        sample_format: SampleFormat::F32,
        channels: target_ch as u16,
        sample_rate: buf.sample_rate,
        num_samples: frames,
        timestamp: buf.timestamp,
    })
}

use super::sample::{bytes_to_f32, f32_to_bytes};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sample::make_test_buffer as make_buffer;
    use std::time::Duration;

    #[test]
    fn stereo_to_mono() {
        // L=1.0, R=0.0 → mono should be 0.5
        let samples = vec![1.0f32, 0.0, 1.0, 0.0, 1.0, 0.0];
        let buf = make_buffer(&samples, 2, 44100);
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();

        assert_eq!(out.channels, 1);
        assert_eq!(out.num_samples, 3);
        let dst = bytes_to_f32(&out.data);
        for &s in dst {
            assert!((s - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn mono_to_stereo() {
        let samples = vec![0.75f32, 0.75, 0.75];
        let buf = make_buffer(&samples, 1, 44100);
        let out = mix_channels(&buf, ChannelLayout::Stereo).unwrap();

        assert_eq!(out.channels, 2);
        assert_eq!(out.num_samples, 3);
        let dst = bytes_to_f32(&out.data);
        for &s in dst {
            assert!((s - 0.75).abs() < 1e-6);
        }
    }

    #[test]
    fn stereo_to_stereo_noop() {
        let samples = vec![1.0f32, -1.0, 0.5, -0.5];
        let buf = make_buffer(&samples, 2, 44100);
        let out = mix_channels(&buf, ChannelLayout::Stereo).unwrap();

        assert_eq!(out.channels, 2);
        assert_eq!(out.data, buf.data);
    }

    #[test]
    fn surround_51_to_stereo() {
        // 5.1: FL=1, FR=0, FC=1, LFE=0.5, SL=0.5, SR=0.5
        let k = std::f32::consts::FRAC_1_SQRT_2;
        let samples = vec![1.0f32, 0.0, 1.0, 0.5, 0.5, 0.5];
        let buf = make_buffer(&samples, 6, 48000);
        let out = mix_channels(&buf, ChannelLayout::Stereo).unwrap();

        assert_eq!(out.channels, 2);
        assert_eq!(out.num_samples, 1);
        let dst = bytes_to_f32(&out.data);
        // L = FL + 0.707*FC + 0.707*SL = 1.0 + 0.707 + 0.354 ≈ 2.061
        let expected_l = 1.0 + k * 1.0 + k * 0.5;
        let expected_r = 0.0 + k * 1.0 + k * 0.5;
        assert!(
            (dst[0] - expected_l).abs() < 1e-4,
            "L: got {}, expected {}",
            dst[0],
            expected_l
        );
        assert!(
            (dst[1] - expected_r).abs() < 1e-4,
            "R: got {}, expected {}",
            dst[1],
            expected_r
        );
    }

    #[test]
    fn surround_51_to_mono() {
        let samples = vec![1.0f32, 1.0, 1.0, 0.0, 0.5, 0.5];
        let buf = make_buffer(&samples, 6, 48000);
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();

        assert_eq!(out.channels, 1);
        assert_eq!(out.num_samples, 1);
        let dst = bytes_to_f32(&out.data);
        assert!(dst[0].abs() > 0.5, "5.1 downmix to mono should have signal");
    }

    #[test]
    fn quad_to_mono() {
        // 4 channels all at 1.0 → mono average = 1.0
        let samples = vec![1.0f32, 1.0, 1.0, 1.0];
        let buf = make_buffer(&samples, 4, 44100);
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();

        assert_eq!(out.channels, 1);
        let dst = bytes_to_f32(&out.data);
        assert!((dst[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn preserves_sample_rate() {
        let samples = vec![1.0f32, -1.0];
        let buf = make_buffer(&samples, 2, 96000);
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();
        assert_eq!(out.sample_rate, 96000);
    }

    #[test]
    fn preserves_frame_count() {
        let frames = 1000;
        let samples = vec![0.5f32; frames * 2];
        let buf = make_buffer(&samples, 2, 44100);
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();
        assert_eq!(out.num_samples, frames);
    }

    #[test]
    fn zero_channels_error() {
        let buf = AudioBuffer {
            data: Bytes::from(vec![0u8; 16]),
            sample_format: SampleFormat::F32,
            channels: 0,
            sample_rate: 44100,
            num_samples: 0,
            timestamp: Duration::ZERO,
        };
        assert!(mix_channels(&buf, ChannelLayout::Mono).is_err());
    }

    #[test]
    fn three_channel_to_mono() {
        // 3ch → mono via generic N→1 path: average all
        let samples = vec![0.3f32, 0.6, 0.9];
        let buf = make_buffer(&samples, 3, 44100);
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();
        assert_eq!(out.channels, 1);
        let dst = bytes_to_f32(&out.data);
        assert!((dst[0] - 0.6).abs() < 1e-5); // (0.3 + 0.6 + 0.9) / 3
    }

    #[test]
    fn three_channel_to_stereo() {
        // 3ch → stereo via generic N→2 path
        let samples = vec![1.0f32, 0.0, 0.5];
        let buf = make_buffer(&samples, 3, 44100);
        let out = mix_channels(&buf, ChannelLayout::Stereo).unwrap();
        assert_eq!(out.channels, 2);
        let dst = bytes_to_f32(&out.data);
        // L = first_ch + extra*gain, R = second_ch + extra*gain
        // extra_gain = 0.5 / (3-2) = 0.5
        // L = 1.0 + 0.5*0.5 = 1.25, R = 0.0 + 0.5*0.5 = 0.25
        assert!((dst[0] - 1.25).abs() < 1e-5);
        assert!((dst[1] - 0.25).abs() < 1e-5);
    }

    #[test]
    fn eight_channel_to_mono() {
        let samples = vec![1.0f32; 8]; // 8ch, 1 frame, all 1.0
        let buf = make_buffer(&samples, 8, 48000);
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();
        assert_eq!(out.channels, 1);
        let dst = bytes_to_f32(&out.data);
        assert!((dst[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn mono_to_mono_noop() {
        let samples = vec![0.42f32, 0.84, 0.21];
        let buf = make_buffer(&samples, 1, 44100);
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();
        assert_eq!(out.channels, 1);
        assert_eq!(out.data, buf.data);
    }

    #[test]
    fn preserves_timestamp() {
        let buf = AudioBuffer {
            data: Bytes::copy_from_slice(f32_to_bytes(&[1.0f32, -1.0])),
            sample_format: SampleFormat::F32,
            channels: 2,
            sample_rate: 44100,
            num_samples: 1,
            timestamp: Duration::from_millis(500),
        };
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();
        assert_eq!(out.timestamp, Duration::from_millis(500));
    }

    #[test]
    fn single_sample_stereo_to_mono() {
        let samples = vec![0.8f32, 0.2];
        let buf = make_buffer(&samples, 2, 44100);
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();
        assert_eq!(out.num_samples, 1);
        let dst = bytes_to_f32(&out.data);
        assert!((dst[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_mix_bounds_validation() {
        // Create a buffer that claims more samples than data actually present.
        // The data has only 2 f32 samples (8 bytes) but num_samples says 100 frames of stereo.
        // With the fix, frame count is derived from data length (1 frame), so this now works.
        let buf = AudioBuffer {
            data: Bytes::copy_from_slice(f32_to_bytes(&[1.0f32, -1.0])),
            sample_format: SampleFormat::F32,
            channels: 2,
            sample_rate: 44100,
            num_samples: 100, // claims 100 frames but data only has 1 frame
            timestamp: Duration::ZERO,
        };
        let result = mix_channels(&buf, ChannelLayout::Mono);
        // Now derives frame count from data (1 frame), should succeed
        assert!(result.is_ok(), "mix should derive frames from data length");
        let out = result.unwrap();
        assert_eq!(out.num_samples, 1);
    }

    /// Regression: shruti benchmark found that stereo→mono with
    /// num_samples set to total interleaved count (not frames) caused
    /// "source buffer too small" validation error.
    #[test]
    fn mix_stereo_to_mono_interleaved_num_samples() {
        // Simulate a buffer where num_samples = total samples (not frames)
        let samples = vec![0.8f32, 0.2, 0.6, 0.4]; // 2 frames stereo
        let buf = AudioBuffer {
            data: Bytes::copy_from_slice(f32_to_bytes(&samples)),
            sample_format: SampleFormat::F32,
            channels: 2,
            sample_rate: 44100,
            num_samples: 4, // BUG: should be 2 frames, but set to 4 total samples
            timestamp: Duration::ZERO,
        };
        // This must not error — should derive frame count from data length
        let out = mix_channels(&buf, ChannelLayout::Mono).unwrap();
        assert_eq!(out.channels, 1);
        assert_eq!(out.num_samples, 2);
        let dst = bytes_to_f32(&out.data);
        assert!((dst[0] - 0.5).abs() < 1e-6); // (0.8 + 0.2) / 2
        assert!((dst[1] - 0.5).abs() < 1e-6); // (0.6 + 0.4) / 2
    }
}
