//! Opus encoder via libopus FFI
//!
//! Wraps the `opus` crate to encode F32 audio into Opus packets.
//! Requires the `opus-enc` feature and libopus system library.

use crate::core::{AudioBuffer, AudioCodec, Result, TarangError};

use super::encode::{AudioEncoder, EncoderConfig};

/// Opus encoder wrapping libopus
pub struct OpusEncoder {
    encoder: opus::Encoder,
    channels: u16,
    frame_size: usize,
    /// Reusable output buffer for encoding (avoids per-frame allocation)
    out_buf: Vec<u8>,
}

impl OpusEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        if config.codec != AudioCodec::Opus {
            return Err(TarangError::UnsupportedCodec(
                "OpusEncoder requires Opus codec".to_string(),
            ));
        }

        let channels = match config.channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            _ => {
                return Err(TarangError::UnsupportedCodec(format!(
                    "Opus supports 1 or 2 channels, got {}",
                    config.channels
                )));
            }
        };

        // Opus requires specific sample rates
        let sample_rate = match config.sample_rate {
            8000 | 12000 | 16000 | 24000 | 48000 => config.sample_rate,
            _ => {
                return Err(TarangError::UnsupportedCodec(format!(
                    "Opus requires 8/12/16/24/48 kHz, got {}",
                    config.sample_rate
                )));
            }
        };

        let encoder = opus::Encoder::new(sample_rate, channels, opus::Application::Audio)
            .map_err(|e| TarangError::Pipeline(format!("failed to create Opus encoder: {e}")))?;

        // Standard Opus frame size: 20ms at the given sample rate
        let frame_size = (sample_rate as usize) / 50; // 20ms

        Ok(Self {
            encoder,
            channels: config.channels,
            frame_size,
            out_buf: vec![0u8; 4000],
        })
    }
}

impl AudioEncoder for OpusEncoder {
    fn encode(&mut self, buf: &AudioBuffer) -> Result<Vec<Vec<u8>>> {
        let float_samples = bytes_to_f32(&buf.data);
        let ch = self.channels as usize;
        let num_frames = buf.num_samples;
        let samples_per_frame = self.frame_size * ch;

        let mut packets = Vec::new();
        let mut offset = 0;

        while offset + self.frame_size <= num_frames {
            let start = offset * ch;
            let end = start + samples_per_frame;
            if end > float_samples.len() {
                break;
            }
            let frame = &float_samples[start..end];

            // Reuse the pre-allocated output buffer
            let len = self
                .encoder
                .encode_float(frame, &mut self.out_buf)
                .map_err(|e| TarangError::Pipeline(format!("Opus encode error: {e}")))?;

            packets.push(self.out_buf[..len].to_vec());
            offset += self.frame_size;
        }

        Ok(packets)
    }

    fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        // Opus doesn't have lookahead buffering in this simple wrapper
        Ok(vec![])
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
        if let Err(e) = OpusEncoder::new(&config) {
            panic!("failed to create 48kHz stereo Opus encoder: {e}");
        }
    }

    #[test]
    fn opus_encoder_creates_mono_48k() {
        let config = opus_config(48000, 1);
        if let Err(e) = OpusEncoder::new(&config) {
            panic!("failed to create 48kHz mono Opus encoder: {e}");
        }
    }

    #[test]
    fn opus_unsupported_sample_rate() {
        let config = opus_config(44100, 2);
        let result = OpusEncoder::new(&config);
        assert!(
            result.is_err(),
            "44100 Hz should be rejected by Opus encoder"
        );
    }

    #[test]
    fn opus_unsupported_sample_rate_22050() {
        let config = opus_config(22050, 1);
        let result = OpusEncoder::new(&config);
        assert!(result.is_err(), "22050 Hz should be rejected");
    }

    #[test]
    fn opus_wrong_codec_rejected() {
        let config = EncoderConfig {
            codec: AudioCodec::Aac,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
        };
        let result = OpusEncoder::new(&config);
        assert!(result.is_err(), "non-Opus codec should be rejected");
    }

    #[test]
    fn opus_unsupported_channel_count() {
        let config = opus_config(48000, 6);
        let result = OpusEncoder::new(&config);
        assert!(result.is_err(), "6-channel Opus should be rejected");
    }

    #[test]
    fn opus_frame_size_48k() {
        let config = opus_config(48000, 2);
        let enc = OpusEncoder::new(&config).unwrap();
        // 48000 / 50 = 960 samples per 20ms frame
        assert_eq!(enc.frame_size, 960, "48kHz frame_size should be 960");
    }

    #[test]
    fn opus_frame_size_16k() {
        let config = opus_config(16000, 1);
        let enc = OpusEncoder::new(&config).unwrap();
        // 16000 / 50 = 320 samples per 20ms frame
        assert_eq!(enc.frame_size, 320, "16kHz frame_size should be 320");
    }

    #[test]
    fn opus_encode_produces_packets() {
        let config = opus_config(48000, 2);
        let mut enc = OpusEncoder::new(&config).unwrap();

        // Generate 2 full frames worth of stereo audio (960 * 2 = 1920 samples)
        let samples = make_sine(1920, 2, 48000);
        let buf = make_buffer(&samples, 2, 48000);
        let packets = enc.encode(&buf).unwrap();
        assert_eq!(
            packets.len(),
            2,
            "1920 samples at 48kHz stereo should produce 2 packets (frame_size=960)"
        );
        for pkt in &packets {
            assert!(!pkt.is_empty(), "encoded Opus packet should not be empty");
        }
    }

    #[test]
    fn opus_partial_frame_no_output() {
        let config = opus_config(48000, 2);
        let mut enc = OpusEncoder::new(&config).unwrap();

        // Generate fewer samples than one frame (960 stereo samples needed, provide 500)
        let samples = make_sine(500, 2, 48000);
        let buf = make_buffer(&samples, 2, 48000);
        let packets = enc.encode(&buf).unwrap();
        assert!(
            packets.is_empty(),
            "partial frame (500 < 960) should produce no packets"
        );
    }

    #[test]
    fn opus_flush_returns_empty() {
        let config = opus_config(48000, 2);
        let mut enc = OpusEncoder::new(&config).unwrap();
        let packets = enc.flush().unwrap();
        assert!(
            packets.is_empty(),
            "Opus flush should return empty (no partial frame buffering)"
        );
    }
}
