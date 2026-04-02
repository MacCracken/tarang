//! AAC encoder via shravan
//!
//! Pure Rust AAC-LC encoder producing ADTS-framed output.
//!
//! # Example
//! ```rust,ignore
//! use tarang::audio::encode_aac::AacEncoder;
//! use tarang::audio::encode::{AudioEncoder, EncoderConfig};
//! use tarang::core::AudioCodec;
//!
//! let config = EncoderConfig::builder(AudioCodec::Aac)
//!     .sample_rate(44100).channels(2).build();
//! let mut enc = AacEncoder::new(&config).unwrap();
//! // let packets = enc.encode(&audio_buf).unwrap();
//! // let adts_data = enc.flush().unwrap(); // ADTS bytes
//! ```

use crate::core::{AudioBuffer, AudioCodec, Result, TarangError};

use super::encode::{AudioEncoder, EncoderConfig};

/// AAC encoder wrapping shravan's AAC-LC implementation.
///
/// Accumulates interleaved F32 samples and produces ADTS-framed AAC
/// output on [`flush`](AudioEncoder::flush).
pub struct AacEncoder {
    channels: u16,
    sample_rate: u32,
    bitrate: u32,
    samples: Vec<f32>,
}

impl AacEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        if config.codec != AudioCodec::Aac {
            return Err(TarangError::UnsupportedCodec(
                "AacEncoder requires Aac codec".into(),
            ));
        }

        if config.channels == 0 || config.channels > 2 {
            return Err(TarangError::UnsupportedCodec(
                format!("AAC supports 1 or 2 channels, got {}", config.channels).into(),
            ));
        }

        Ok(Self {
            channels: config.channels,
            sample_rate: config.sample_rate,
            bitrate: 128_000,
            samples: Vec::new(),
        })
    }
}

impl AudioEncoder for AacEncoder {
    fn encode(&mut self, buf: &AudioBuffer) -> Result<Vec<Vec<u8>>> {
        let float_samples = bytes_to_f32(&buf.data);
        self.samples.extend_from_slice(float_samples);
        Ok(vec![])
    }

    fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        if self.samples.is_empty() {
            return Ok(vec![]);
        }

        let samples = std::mem::take(&mut self.samples);
        let encoded = shravan::aac::encode(&samples, self.sample_rate, self.channels, self.bitrate)
            .map_err(|e| TarangError::EncodeError(format!("AAC encode error: {e}").into()))?;

        Ok(vec![encoded])
    }
}

use super::sample::bytes_to_f32;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_buffer(samples: &[f32], channels: u16, sample_rate: u32) -> crate::core::AudioBuffer {
        crate::audio::sample::make_test_buffer(samples, channels, sample_rate)
    }

    fn make_sine(num_samples: usize, channels: u16, sample_rate: u32) -> Vec<f32> {
        crate::audio::sample::make_test_sine(440.0, sample_rate, num_samples, channels)
    }

    fn aac_config(sample_rate: u32, channels: u16) -> EncoderConfig {
        EncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate,
            channels,
            bits_per_sample: 16,
        }
    }

    #[test]
    fn aac_encoder_creates_stereo() {
        assert!(AacEncoder::new(&aac_config(44100, 2)).is_ok());
    }

    #[test]
    fn aac_encoder_creates_mono() {
        assert!(AacEncoder::new(&aac_config(44100, 1)).is_ok());
    }

    #[test]
    fn aac_encode_produces_output() {
        let config = aac_config(44100, 2);
        let mut enc = AacEncoder::new(&config).unwrap();

        let samples = make_sine(4096, 2, 44100);
        let buf = make_buffer(&samples, 2, 44100);
        enc.encode(&buf).unwrap();

        let result = enc.flush().unwrap();
        assert_eq!(result.len(), 1);
        assert!(!result[0].is_empty());
    }

    #[test]
    fn aac_unsupported_channel_count() {
        assert!(AacEncoder::new(&aac_config(44100, 6)).is_err());
    }

    #[test]
    fn aac_zero_channels_rejected() {
        assert!(AacEncoder::new(&aac_config(44100, 0)).is_err());
    }

    #[test]
    fn aac_wrong_codec_rejected() {
        let config = EncoderConfig {
            codec: AudioCodec::Opus,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        assert!(AacEncoder::new(&config).is_err());
    }

    #[test]
    fn aac_flush_empty_returns_empty() {
        let mut enc = AacEncoder::new(&aac_config(44100, 2)).unwrap();
        let result = enc.flush().unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn aac_flush_after_encode() {
        let mut enc = AacEncoder::new(&aac_config(44100, 2)).unwrap();
        let samples = make_sine(4096, 2, 44100);
        let buf = make_buffer(&samples, 2, 44100);
        enc.encode(&buf).unwrap();
        assert!(enc.flush().is_ok());
    }
}
