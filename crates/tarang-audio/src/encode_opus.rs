//! Opus encoder via libopus FFI
//!
//! Wraps the `opus` crate to encode F32 audio into Opus packets.
//! Requires the `opus-enc` feature and libopus system library.

use tarang_core::{AudioBuffer, AudioCodec, Result, TarangError};

use crate::encode::{AudioEncoder, EncoderConfig};

/// Opus encoder wrapping libopus
pub struct OpusEncoder {
    encoder: opus::Encoder,
    channels: u16,
    frame_size: usize,
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

            // Opus can produce up to 4000 bytes per frame
            let mut out = vec![0u8; 4000];
            let len = self
                .encoder
                .encode_float(frame, &mut out)
                .map_err(|e| TarangError::Pipeline(format!("Opus encode error: {e}")))?;

            out.truncate(len);
            packets.push(out);
            offset += self.frame_size;
        }

        Ok(packets)
    }

    fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        // Opus doesn't have lookahead buffering in this simple wrapper
        Ok(vec![])
    }
}

use crate::sample::bytes_to_f32;
