//! AAC decoder via shravan
//!
//! Decodes ADTS-framed AAC streams into interleaved F32 audio buffers.
//!
//! # Example
//! ```rust,ignore
//! use tarang::audio::decode_aac::AacDecoder;
//!
//! let adts_data = std::fs::read("audio.aac").unwrap();
//! let (info, samples) = AacDecoder::decode_adts(&adts_data).unwrap();
//! println!("{}Hz, {} channels, {} samples", info.sample_rate, info.channels, samples.len());
//! ```

use crate::core::{AudioBuffer, Result, SampleFormat, TarangError};
use bytes::Bytes;
use std::time::Duration;

/// AAC decoder backed by shravan.
///
/// Provides ADTS stream decoding. For raw AAC frames from MP4 containers,
/// use [`decode_frame`](Self::decode_frame) which wraps each frame in an
/// ADTS header before decoding.
pub struct AacDecoder {
    sample_rate: u32,
    channels: u16,
}

impl AacDecoder {
    /// Create a decoder with known stream parameters (from MP4 container metadata).
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }

    /// Decode a complete ADTS-framed AAC stream.
    pub fn decode_adts(data: &[u8]) -> Result<(shravan::FormatInfo, Vec<f32>)> {
        shravan::aac::decode(data)
            .map_err(|e| TarangError::DecodeError(format!("AAC decode error: {e}").into()))
    }

    /// Decode a single raw AAC frame into an [`AudioBuffer`].
    ///
    /// Wraps the raw frame in an ADTS header using the stream parameters
    /// provided at construction, then decodes via shravan.
    pub fn decode_frame(&self, frame_data: &[u8], timestamp: Duration) -> Result<AudioBuffer> {
        if frame_data.is_empty() {
            return Err(TarangError::DecodeError("empty AAC frame".into()));
        }

        // Build ADTS header for this raw frame
        let adts = build_adts_frame(self.sample_rate, self.channels, frame_data);

        let (_info, samples) = shravan::aac::decode(&adts)
            .map_err(|e| TarangError::DecodeError(format!("AAC frame decode error: {e}").into()))?;

        let num_frames = samples.len() / self.channels.max(1) as usize;
        let byte_data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();

        Ok(AudioBuffer {
            data: Bytes::from(byte_data),
            sample_format: SampleFormat::F32,
            channels: self.channels,
            sample_rate: self.sample_rate,
            num_frames,
            timestamp,
        })
    }
}

/// Build a 7-byte ADTS header wrapping a raw AAC frame.
fn build_adts_frame(sample_rate: u32, channels: u16, frame_data: &[u8]) -> Vec<u8> {
    let freq_index = match sample_rate {
        96000 => 0u8,
        88200 => 1,
        64000 => 2,
        48000 => 3,
        44100 => 4,
        32000 => 5,
        24000 => 6,
        22050 => 7,
        16000 => 8,
        12000 => 9,
        11025 => 10,
        8000 => 11,
        _ => 4, // default to 44100
    };
    let chan_config = channels.min(7) as u8;
    let frame_len = (frame_data.len() + 7) as u16; // 7 = ADTS header size

    let mut header = [0u8; 7];
    // Syncword (12 bits) + ID (1) + Layer (2) + Protection absent (1)
    header[0] = 0xFF;
    header[1] = 0xF1; // MPEG-4, Layer 0, no CRC
    // Profile (2) + Sampling freq index (4) + Private (1) + Channel config high (1)
    header[2] = (1 << 6) | (freq_index << 2) | (chan_config >> 2);
    // Channel config low (2) + Original (1) + Home (1) + Copyright ID (1) + Copyright start (1) + Frame length high (2)
    header[3] = ((chan_config & 0x03) << 6) | ((frame_len >> 11) as u8 & 0x03);
    header[4] = (frame_len >> 3) as u8;
    header[5] = ((frame_len & 0x07) as u8) << 5 | 0x1F;
    header[6] = 0xFC;

    let mut adts = Vec::with_capacity(7 + frame_data.len());
    adts.extend_from_slice(&header);
    adts.extend_from_slice(frame_data);
    adts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_decoder() {
        let _dec = AacDecoder::new(44100, 2);
    }

    #[test]
    fn decode_empty_frame_errors() {
        let dec = AacDecoder::new(44100, 2);
        let result = dec.decode_frame(&[], Duration::ZERO);
        assert!(result.is_err());
    }

    #[test]
    fn adts_header_correct_length() {
        let frame = vec![0u8; 100];
        let adts = build_adts_frame(44100, 2, &frame);
        assert_eq!(adts.len(), 107); // 7 header + 100 data
        assert_eq!(adts[0], 0xFF);
        assert_eq!(adts[1], 0xF1);
    }

    #[test]
    fn adts_header_syncword() {
        let adts = build_adts_frame(48000, 1, &[0u8; 10]);
        // Syncword is 0xFFF
        assert_eq!(adts[0], 0xFF);
        assert_eq!(adts[1] & 0xF0, 0xF0);
    }
}
