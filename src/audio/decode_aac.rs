//! AAC decoder via fdk-aac FFI
//!
//! Optional alternative to symphonia's built-in AAC decoder.
//! May offer better quality for HE-AAC and HE-AACv2 profiles.
//! Requires the `aac-dec` feature and the `libfdk-aac` system library
//! (LGPL-2.1).
//!
//! **System dependency**: install `libfdk-aac-dev` (Debian/Ubuntu),
//! `fdk-aac-devel` (Fedora), or `fdk-aac` (Arch) before enabling this
//! feature.

use crate::core::{AudioBuffer, Result, SampleFormat, TarangError};
use bytes::Bytes;
use std::time::Duration;

/// Maximum decoded frame size: 2048 samples * 8 channels.
const MAX_PCM_SAMPLES: usize = 2048 * 8;

/// AAC decoder backed by fdk-aac (libfdk-aac FFI).
///
/// Decodes AAC frames into interleaved F32 audio buffers.
/// Use [`FdkAacDecoder::new_raw`] for raw AAC packets (e.g. from MP4)
/// or [`FdkAacDecoder::new_adts`] for ADTS-framed streams.
pub struct FdkAacDecoder {
    decoder: fdk_aac::dec::Decoder,
    pcm_buf: Vec<i16>,
}

impl FdkAacDecoder {
    /// Create a decoder for raw AAC packets (e.g. from MP4 container).
    pub fn new_raw() -> Self {
        Self {
            decoder: fdk_aac::dec::Decoder::new(fdk_aac::dec::Transport::Raw),
            pcm_buf: vec![0i16; MAX_PCM_SAMPLES],
        }
    }

    /// Create a decoder for ADTS-framed AAC streams.
    pub fn new_adts() -> Self {
        Self {
            decoder: fdk_aac::dec::Decoder::new(fdk_aac::dec::Transport::Adts),
            pcm_buf: vec![0i16; MAX_PCM_SAMPLES],
        }
    }

    /// Configure the decoder with an AudioSpecificConfig blob (from MP4 esds box).
    pub fn configure(&mut self, audio_specific_config: &[u8]) -> Result<()> {
        self.decoder
            .config_raw(audio_specific_config)
            .map_err(|e| TarangError::DecodeError(format!("FDK-AAC config failed: {e}").into()))
    }

    /// Decode a single AAC packet into an [`AudioBuffer`].
    ///
    /// Call [`fill`](Self::fill) first, then `decode_frame` to get the output.
    /// The first successful call determines sample rate and channel count
    /// from the stream info.
    pub fn decode(&mut self, data: &[u8], timestamp: Duration) -> Result<AudioBuffer> {
        // Fill the decoder's internal bitstream buffer
        let _consumed = self
            .decoder
            .fill(data)
            .map_err(|e| TarangError::DecodeError(format!("FDK-AAC fill failed: {e}").into()))?;

        // Decode one frame
        self.decoder
            .decode_frame(&mut self.pcm_buf)
            .map_err(|e| TarangError::DecodeError(format!("FDK-AAC decode failed: {e}").into()))?;

        // Get stream info
        let info = self.decoder.stream_info();
        let channels = info.numChannels as u16;
        let sample_rate = info.sampleRate as u32;
        let frame_size = self.decoder.decoded_frame_size();

        if channels == 0 || sample_rate == 0 || frame_size == 0 {
            return Err(TarangError::DecodeError(
                "FDK-AAC decoded frame has invalid stream info".into(),
            ));
        }

        let num_frames = frame_size / channels as usize;

        // Convert i16 PCM to f32
        let mut f32_data = Vec::with_capacity(frame_size);
        for &s in &self.pcm_buf[..frame_size] {
            f32_data.push(s as f32 / 32768.0);
        }

        let byte_data: Vec<u8> = f32_data.iter().flat_map(|s| s.to_le_bytes()).collect();

        Ok(AudioBuffer {
            data: Bytes::from(byte_data),
            sample_format: SampleFormat::F32,
            channels,
            sample_rate,
            num_frames,
            timestamp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_raw_decoder() {
        let _dec = FdkAacDecoder::new_raw();
    }

    #[test]
    fn create_adts_decoder() {
        let _dec = FdkAacDecoder::new_adts();
    }

    #[test]
    fn decode_invalid_data_errors() {
        let mut dec = FdkAacDecoder::new_raw();
        let result = dec.decode(&[0xFF, 0x00, 0x42], Duration::ZERO);
        assert!(result.is_err());
    }

    #[test]
    fn decode_empty_errors() {
        let mut dec = FdkAacDecoder::new_raw();
        let result = dec.decode(&[], Duration::ZERO);
        assert!(result.is_err());
    }
}
