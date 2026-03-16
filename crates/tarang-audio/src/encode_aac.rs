//! AAC encoder via fdk-aac FFI
//!
//! Wraps the `fdk-aac` crate to encode F32 audio into AAC packets.
//! Requires the `aac-enc` feature and libfdk-aac system library.

use tarang_core::{AudioBuffer, AudioCodec, Result, TarangError};

use crate::encode::{AudioEncoder, EncoderConfig};

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
        })
        .map_err(|e| TarangError::Pipeline(format!("failed to create AAC encoder: {e:?}")))?;

        Ok(Self {
            encoder,
            channels: config.channels,
            buf_i16: Vec::new(),
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
            self.buf_i16.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
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
        match self.encoder.encode(empty, &mut out) {
            Ok(result) if result.output_size > 0 => {
                out.truncate(result.output_size);
                Ok(vec![out])
            }
            _ => Ok(vec![]),
        }
    }
}

fn bytes_to_f32(bytes: &[u8]) -> &[f32] {
    assert!(bytes.len() % 4 == 0);
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, bytes.len() / 4) }
}
