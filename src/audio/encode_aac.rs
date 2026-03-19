//! AAC encoder via fdk-aac FFI
//!
//! Wraps the `fdk-aac` crate to encode F32 audio into AAC packets.
//! Requires the `aac-enc` feature and libfdk-aac system library.

use crate::core::{AudioBuffer, AudioCodec, Result, TarangError};

use super::encode::{AudioEncoder, EncoderConfig};

/// AAC encoder wrapping fdk-aac
pub struct AacEncoder {
    encoder: fdk_aac::enc::Encoder,
    channels: u16,
    buf_i16: Vec<i16>,
}

impl AacEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        if config.codec != AudioCodec::Aac {
            return Err(TarangError::UnsupportedCodec(
                "AacEncoder requires Aac codec".to_string(),
            ));
        }

        let encoder = fdk_aac::enc::Encoder::new(fdk_aac::enc::EncoderParams {
            bit_rate: fdk_aac::enc::BitRate::Cbr(128000),
            sample_rate: config.sample_rate,
            transport: fdk_aac::enc::Transport::Raw,
            channels: match config.channels {
                1 => fdk_aac::enc::ChannelMode::Mono,
                2 => fdk_aac::enc::ChannelMode::Stereo,
                _ => {
                    return Err(TarangError::UnsupportedCodec(format!(
                        "AAC supports 1 or 2 channels, got {}",
                        config.channels
                    )));
                }
            },
            audio_object_type: fdk_aac::enc::AudioObjectType::Mpeg4LowComplexity,
        })
        .map_err(|e| TarangError::Pipeline(format!("failed to create AAC encoder: {e:?}")))?;

        // Pre-allocate i16 buffer for a typical frame size (1024 samples per channel)
        let initial_capacity = 1024 * config.channels as usize;
        Ok(Self {
            encoder,
            channels: config.channels,
            buf_i16: Vec::with_capacity(initial_capacity),
        })
    }
}

impl AudioEncoder for AacEncoder {
    fn encode(&mut self, buf: &AudioBuffer) -> Result<Vec<Vec<u8>>> {
        let float_samples = bytes_to_f32(&buf.data);
        let total = buf.num_samples * self.channels as usize;

        // fdk-aac expects interleaved i16
        self.buf_i16.clear();
        self.buf_i16.reserve(total);
        for &s in &float_samples[..total.min(float_samples.len())] {
            self.buf_i16
                .push((s.clamp(-1.0, 1.0) * crate::audio::sample::I16_SCALE) as i16);
        }

        let mut packets = Vec::new();

        // fdk-aac encodes in chunks of its internal frame size
        let info = self
            .encoder
            .info()
            .map_err(|e| TarangError::Pipeline(format!("AAC encoder info error: {e:?}")))?;
        let frame_size = info.frameLength as usize * self.channels as usize;

        let mut offset = 0;
        while offset + frame_size <= self.buf_i16.len() {
            let mut out = vec![0u8; 2048];
            let result = self
                .encoder
                .encode(&self.buf_i16[offset..offset + frame_size], &mut out)
                .map_err(|e| TarangError::Pipeline(format!("AAC encode error: {e:?}")))?;

            if result.output_size > 0 {
                out.truncate(result.output_size);
                packets.push(out);
            }
            offset += frame_size;
        }

        Ok(packets)
    }

    fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        // Flush remaining encoder delay
        let mut out = vec![0u8; 2048];
        let empty: &[i16] = &[];
        let result = self
            .encoder
            .encode(empty, &mut out)
            .map_err(|e| TarangError::Pipeline(format!("AAC flush error: {e:?}")))?;

        if result.output_size > 0 {
            out.truncate(result.output_size);
            Ok(vec![out])
        } else {
            Ok(vec![])
        }
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
        let config = aac_config(44100, 2);
        if let Err(e) = AacEncoder::new(&config) {
            panic!("failed to create stereo AAC encoder: {e}");
        }
    }

    #[test]
    fn aac_encoder_creates_mono() {
        let config = aac_config(44100, 1);
        if let Err(e) = AacEncoder::new(&config) {
            panic!("failed to create mono AAC encoder: {e}");
        }
    }

    #[test]
    fn aac_encode_produces_output() {
        let config = aac_config(44100, 2);
        let mut enc = AacEncoder::new(&config).unwrap();

        // Generate enough samples to fill at least one AAC frame (typically 1024 samples)
        let samples = make_sine(4096, 2, 44100);
        let buf = make_buffer(&samples, 2, 44100);
        let packets = enc.encode(&buf).unwrap();
        assert!(
            !packets.is_empty(),
            "encoding 4096 stereo samples should produce at least one packet"
        );
        for pkt in &packets {
            assert!(!pkt.is_empty(), "encoded packet should not be empty");
        }
    }

    #[test]
    fn aac_unsupported_channel_count() {
        let config = EncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate: 44100,
            channels: 6,
            bits_per_sample: 16,
        };
        let result = AacEncoder::new(&config);
        assert!(result.is_err(), "6-channel AAC should be rejected");
    }

    #[test]
    fn aac_wrong_codec_rejected() {
        let config = EncoderConfig {
            codec: AudioCodec::Opus,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let result = AacEncoder::new(&config);
        assert!(result.is_err(), "non-AAC codec should be rejected");
    }

    #[test]
    fn aac_flush_does_not_panic() {
        let config = aac_config(44100, 2);
        let mut enc = AacEncoder::new(&config).unwrap();
        // Flush without encoding anything should succeed
        let result = enc.flush();
        assert!(result.is_ok(), "flush should not error: {result:?}");
    }

    #[test]
    fn aac_flush_after_encode() {
        let config = aac_config(44100, 2);
        let mut enc = AacEncoder::new(&config).unwrap();

        let samples = make_sine(4096, 2, 44100);
        let buf = make_buffer(&samples, 2, 44100);
        let _ = enc.encode(&buf).unwrap();

        let flush_result = enc.flush();
        assert!(flush_result.is_ok(), "flush after encode should succeed");
    }
}
