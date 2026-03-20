//! Audio encoding — convert decoded AudioBuffers into codec-specific byte streams
//!
//! Encoders take F32 interleaved audio and produce encoded packets suitable
//! for writing into container muxers.

use crate::core::{AudioBuffer, AudioCodec, Result, TarangError};

/// Configuration for an audio encoder.
///
/// Use [`EncoderConfig::builder`] for a guided construction, or create
/// directly for simple cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    /// Target audio codec.
    pub codec: AudioCodec,
    /// Output sample rate in Hz (e.g. 44100, 48000).
    pub sample_rate: u32,
    /// Number of output channels (1 = mono, 2 = stereo).
    pub channels: u16,
    /// Bits per sample for PCM/FLAC (16, 24, or 32). Ignored by Opus/AAC.
    pub bits_per_sample: u16,
}

impl EncoderConfig {
    /// Create a builder for constructing an encoder config.
    pub fn builder(codec: AudioCodec) -> EncoderConfigBuilder {
        EncoderConfigBuilder {
            codec,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        }
    }
}

/// Builder for [`EncoderConfig`].
#[derive(Debug, Clone)]
pub struct EncoderConfigBuilder {
    codec: AudioCodec,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
}

impl EncoderConfigBuilder {
    /// Set the sample rate in Hz (default: 44100).
    pub fn sample_rate(mut self, rate: u32) -> Self {
        self.sample_rate = rate;
        self
    }

    /// Set the number of channels (default: 2).
    pub fn channels(mut self, ch: u16) -> Self {
        self.channels = ch;
        self
    }

    /// Set bits per sample for PCM/FLAC (default: 16).
    pub fn bits_per_sample(mut self, bits: u16) -> Self {
        self.bits_per_sample = bits;
        self
    }

    /// Build the config.
    pub fn build(self) -> EncoderConfig {
        EncoderConfig {
            codec: self.codec,
            sample_rate: self.sample_rate,
            channels: self.channels,
            bits_per_sample: self.bits_per_sample,
        }
    }
}

/// Trait for audio encoders
pub trait AudioEncoder {
    /// Encode an audio buffer into one or more packets of encoded data.
    fn encode(&mut self, buf: &AudioBuffer) -> Result<Vec<Vec<u8>>>;

    /// Flush any remaining buffered data (encoder delay).
    fn flush(&mut self) -> Result<Vec<Vec<u8>>>;
}

/// PCM encoder — converts F32 samples to interleaved integer PCM.
/// Used for writing WAV files.
pub struct PcmEncoder {
    bits_per_sample: u16,
    channels: u16,
}

impl PcmEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        match config.bits_per_sample {
            16 | 24 | 32 => {}
            other => {
                return Err(TarangError::UnsupportedCodec(
                    format!("PCM encoder: unsupported bits_per_sample {other}").into(),
                ));
            }
        }
        Ok(Self {
            bits_per_sample: config.bits_per_sample,
            channels: config.channels,
        })
    }
}

impl AudioEncoder for PcmEncoder {
    fn encode(&mut self, buf: &AudioBuffer) -> Result<Vec<Vec<u8>>> {
        let samples = bytes_to_f32(&buf.data);
        let expected = buf
            .num_samples
            .checked_mul(self.channels as usize)
            .ok_or_else(|| {
                TarangError::Pipeline(
                    format!(
                        "overflow computing expected samples: {} * {}",
                        buf.num_samples, self.channels
                    )
                    .into(),
                )
            })?;
        if samples.len() < expected {
            return Err(TarangError::Pipeline(
                format!(
                    "buffer has {} samples but expected {}",
                    samples.len(),
                    expected
                )
                .into(),
            ));
        }

        let mut out = Vec::with_capacity(expected * (self.bits_per_sample as usize / 8));

        match self.bits_per_sample {
            16 => {
                for &s in &samples[..expected] {
                    let clamped = s.clamp(-1.0, 1.0);
                    let i = (clamped * sample::I16_SCALE) as i16;
                    out.extend_from_slice(&i.to_le_bytes());
                }
            }
            24 => {
                for &s in &samples[..expected] {
                    let clamped = s.clamp(-1.0, 1.0);
                    let i = (clamped * sample::I24_SCALE) as i32;
                    let bytes = i.to_le_bytes();
                    out.extend_from_slice(&bytes[..3]);
                }
            }
            32 => {
                for &s in &samples[..expected] {
                    let clamped = s.clamp(-1.0, 1.0);
                    let i = (clamped * sample::I32_SCALE) as i32;
                    out.extend_from_slice(&i.to_le_bytes());
                }
            }
            _ => unreachable!(),
        }

        Ok(vec![out])
    }

    fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        Ok(vec![])
    }
}

/// Create an encoder for the given codec.
pub fn create_encoder(config: &EncoderConfig) -> Result<Box<dyn AudioEncoder>> {
    match config.codec {
        AudioCodec::Pcm => Ok(Box::new(PcmEncoder::new(config)?)),
        AudioCodec::Flac => Ok(Box::new(super::FlacEncoder::new(config)?)),
        #[cfg(feature = "opus-enc")]
        AudioCodec::Opus => Ok(Box::new(super::OpusEncoder::new(config)?)),
        #[cfg(feature = "aac-enc")]
        AudioCodec::Aac => Ok(Box::new(super::AacEncoder::new(config)?)),
        other => Err(TarangError::UnsupportedCodec(
            format!("no encoder for {other}").into(),
        )),
    }
}

use super::sample::{self, bytes_to_f32};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sample::make_test_buffer as make_buffer;

    #[test]
    fn pcm_encode_16bit() {
        let config = EncoderConfig {
            codec: AudioCodec::Pcm,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };
        let mut enc = PcmEncoder::new(&config).unwrap();

        // Encode a known value: 0.5 → 16383 (approximately)
        let buf = make_buffer(&[0.5f32, -0.5, 0.0, 1.0], 1, 44100);
        let packets = enc.encode(&buf).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].len(), 8); // 4 samples * 2 bytes

        // Verify first sample
        let s = i16::from_le_bytes(packets[0][0..2].try_into().unwrap());
        assert!((s - 16383).abs() <= 1, "expected ~16383, got {s}");

        // Verify negative
        let s_neg = i16::from_le_bytes(packets[0][2..4].try_into().unwrap());
        assert!((s_neg + 16383).abs() <= 1, "expected ~-16383, got {s_neg}");

        // Verify zero
        let s_zero = i16::from_le_bytes(packets[0][4..6].try_into().unwrap());
        assert_eq!(s_zero, 0);

        // Verify clamp at 1.0
        let s_max = i16::from_le_bytes(packets[0][6..8].try_into().unwrap());
        assert_eq!(s_max, 32767);
    }

    #[test]
    fn pcm_encode_24bit() {
        let config = EncoderConfig {
            codec: AudioCodec::Pcm,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 24,
        };
        let mut enc = PcmEncoder::new(&config).unwrap();

        let buf = make_buffer(&[0.0f32], 1, 44100);
        let packets = enc.encode(&buf).unwrap();
        assert_eq!(packets[0].len(), 3); // 1 sample * 3 bytes
        assert_eq!(&packets[0], &[0, 0, 0]);
    }

    #[test]
    fn pcm_encode_32bit() {
        let config = EncoderConfig {
            codec: AudioCodec::Pcm,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 32,
        };
        let mut enc = PcmEncoder::new(&config).unwrap();

        let buf = make_buffer(&[1.0f32], 1, 44100);
        let packets = enc.encode(&buf).unwrap();
        assert_eq!(packets[0].len(), 4);
        let s = i32::from_le_bytes(packets[0][0..4].try_into().unwrap());
        assert_eq!(s, 2147483647);
    }

    #[test]
    fn pcm_encode_stereo() {
        let config = EncoderConfig {
            codec: AudioCodec::Pcm,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut enc = PcmEncoder::new(&config).unwrap();

        // 2 frames * 2 channels
        let buf = make_buffer(&[0.5f32, -0.5, 0.25, -0.25], 2, 44100);
        let packets = enc.encode(&buf).unwrap();
        assert_eq!(packets[0].len(), 8); // 4 samples * 2 bytes
    }

    #[test]
    fn create_pcm_encoder() {
        let config = EncoderConfig {
            codec: AudioCodec::Pcm,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let enc = create_encoder(&config);
        assert!(enc.is_ok());
    }

    #[test]
    fn create_unsupported_encoder() {
        let config = EncoderConfig {
            codec: AudioCodec::Wma,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let enc = create_encoder(&config);
        assert!(enc.is_err());
    }

    #[test]
    fn create_flac_encoder() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let enc = create_encoder(&config);
        assert!(enc.is_ok());
    }

    #[test]
    fn pcm_flush_empty() {
        let config = EncoderConfig {
            codec: AudioCodec::Pcm,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };
        let mut enc = PcmEncoder::new(&config).unwrap();
        let packets = enc.flush().unwrap();
        assert!(packets.is_empty());
    }

    #[test]
    fn encoder_config_builder_defaults() {
        let config = EncoderConfig::builder(AudioCodec::Flac).build();
        assert_eq!(config.codec, AudioCodec::Flac);
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.channels, 2);
        assert_eq!(config.bits_per_sample, 16);
    }

    #[test]
    fn encoder_config_builder_custom() {
        let config = EncoderConfig::builder(AudioCodec::Pcm)
            .sample_rate(96000)
            .channels(1)
            .bits_per_sample(24)
            .build();
        assert_eq!(config.codec, AudioCodec::Pcm);
        assert_eq!(config.sample_rate, 96000);
        assert_eq!(config.channels, 1);
        assert_eq!(config.bits_per_sample, 24);
    }

    #[test]
    fn encoder_config_builder_creates_working_encoder() {
        let config = EncoderConfig::builder(AudioCodec::Pcm)
            .sample_rate(48000)
            .channels(2)
            .bits_per_sample(16)
            .build();
        let enc = create_encoder(&config);
        assert!(enc.is_ok());
    }
}
