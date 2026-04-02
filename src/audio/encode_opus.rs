//! Opus encoder via shravan
//!
//! Wraps shravan's Opus CELT-mode encoder to produce Ogg/Opus output.
//! Requires the `opus-enc` feature.
//!
//! # Example
//! ```rust,ignore
//! use tarang::audio::encode_opus::OpusEncoder;
//! use tarang::audio::encode::{AudioEncoder, EncoderConfig};
//! use tarang::core::AudioCodec;
//!
//! let config = EncoderConfig::builder(AudioCodec::Opus)
//!     .sample_rate(48000).channels(2).build();
//! let mut enc = OpusEncoder::new(&config).unwrap();
//! // let packets = enc.encode(&audio_buf).unwrap();
//! // let final_data = enc.flush().unwrap(); // Ogg/Opus bytes
//! ```

use crate::core::{AudioBuffer, AudioCodec, Result, TarangError};

use super::encode::{AudioEncoder, EncoderConfig};

/// Opus encoder wrapping shravan's CELT-mode implementation.
///
/// Accumulates interleaved F32 samples and produces a complete Ogg/Opus
/// stream on [`flush`](AudioEncoder::flush).
pub struct OpusEncoder {
    channels: u16,
    sample_rate: u32,
    bitrate: u32,
    /// Accumulated samples across encode() calls
    samples: Vec<f32>,
    /// 20ms frame size in samples (per channel).
    #[allow(dead_code)]
    frame_size: usize,
}

impl OpusEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        if config.codec != AudioCodec::Opus {
            return Err(TarangError::UnsupportedCodec(
                "OpusEncoder requires Opus codec".into(),
            ));
        }

        if config.channels == 0 || config.channels > 2 {
            return Err(TarangError::UnsupportedCodec(
                format!("Opus supports 1 or 2 channels, got {}", config.channels).into(),
            ));
        }

        // shravan's Opus encoder currently requires 48kHz
        if config.sample_rate != 48000 {
            return Err(TarangError::UnsupportedCodec(
                format!("Opus encoder requires 48 kHz, got {}", config.sample_rate).into(),
            ));
        }

        let frame_size = (config.sample_rate as usize) / 50;

        Ok(Self {
            channels: config.channels,
            sample_rate: config.sample_rate,
            bitrate: 128_000,
            samples: Vec::new(),
            frame_size,
        })
    }
}

impl AudioEncoder for OpusEncoder {
    fn encode(&mut self, buf: &AudioBuffer) -> Result<Vec<Vec<u8>>> {
        let float_samples = bytes_to_f32(&buf.data);
        self.samples.extend_from_slice(float_samples);
        // Samples are accumulated and encoded on flush
        Ok(vec![])
    }

    fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        if self.samples.is_empty() {
            return Ok(vec![]);
        }

        let samples = std::mem::take(&mut self.samples);
        let encoded =
            shravan::opus::encode(&samples, self.sample_rate, self.channels, self.bitrate)
                .map_err(|e| TarangError::EncodeError(format!("Opus encode error: {e}").into()))?;

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

    fn opus_config(sample_rate: u32, channels: u16) -> EncoderConfig {
        EncoderConfig {
            codec: AudioCodec::Opus,
            sample_rate,
            channels,
            bits_per_sample: 16,
        }
    }

    #[test]
    fn opus_encoder_creates_stereo_48k() {
        let config = opus_config(48000, 2);
        assert!(OpusEncoder::new(&config).is_ok());
    }

    #[test]
    fn opus_encoder_creates_mono_48k() {
        let config = opus_config(48000, 1);
        assert!(OpusEncoder::new(&config).is_ok());
    }

    #[test]
    fn opus_unsupported_sample_rate() {
        let config = opus_config(44100, 2);
        assert!(OpusEncoder::new(&config).is_err());
    }

    #[test]
    fn opus_unsupported_sample_rate_22050() {
        let config = opus_config(22050, 1);
        assert!(OpusEncoder::new(&config).is_err());
    }

    #[test]
    fn opus_wrong_codec_rejected() {
        let config = EncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
        };
        assert!(OpusEncoder::new(&config).is_err());
    }

    #[test]
    fn opus_unsupported_channel_count() {
        let config = opus_config(48000, 6);
        assert!(OpusEncoder::new(&config).is_err());
    }

    #[test]
    fn opus_zero_channels_rejected() {
        let config = opus_config(48000, 0);
        assert!(OpusEncoder::new(&config).is_err());
    }

    #[test]
    fn opus_frame_size_48k() {
        let config = opus_config(48000, 2);
        let enc = OpusEncoder::new(&config).unwrap();
        assert_eq!(enc.frame_size, 960);
    }

    #[test]
    fn opus_encode_accumulates_then_flush() {
        let config = opus_config(48000, 2);
        let mut enc = OpusEncoder::new(&config).unwrap();

        let samples = make_sine(1920, 2, 48000);
        let buf = make_buffer(&samples, 2, 48000);

        // encode accumulates — returns empty
        let packets = enc.encode(&buf).unwrap();
        assert!(packets.is_empty());

        // flush produces Ogg/Opus output
        let result = enc.flush().unwrap();
        assert_eq!(result.len(), 1);
        assert!(!result[0].is_empty());
        // Ogg magic bytes
        assert_eq!(&result[0][..4], b"OggS");
    }

    #[test]
    fn opus_flush_empty_returns_empty() {
        let config = opus_config(48000, 2);
        let mut enc = OpusEncoder::new(&config).unwrap();
        let packets = enc.flush().unwrap();
        assert!(packets.is_empty());
    }

    #[test]
    fn opus_encode_mono() {
        let config = opus_config(48000, 1);
        let mut enc = OpusEncoder::new(&config).unwrap();

        let samples = make_sine(960, 1, 48000);
        let buf = make_buffer(&samples, 1, 48000);
        enc.encode(&buf).unwrap();

        let result = enc.flush().unwrap();
        assert_eq!(result.len(), 1);
        assert!(!result[0].is_empty());
    }
}
