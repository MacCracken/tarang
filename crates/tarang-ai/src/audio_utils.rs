//! Shared audio sample format conversion utilities
//!
//! Common routines for extracting mono F32 samples from AudioBuffers
//! and converting between sample formats. Used by both fingerprinting
//! and transcription.

use tarang_core::{AudioBuffer, Result, SampleFormat, TarangError};

/// Extract mono F32 samples from an AudioBuffer.
///
/// For multi-channel buffers, only the first channel is extracted.
/// Supports F32 and I16 input formats.
pub fn extract_mono_f32(buf: &AudioBuffer) -> Result<Vec<f32>> {
    let channels = buf.channels as usize;
    match buf.sample_format {
        SampleFormat::F32 => {
            let total_values = buf.data.len() / 4;
            let num_samples = total_values / channels;
            let mut mono = Vec::with_capacity(num_samples);
            for i in 0..num_samples {
                let offset = i * channels * 4;
                if offset + 4 <= buf.data.len() {
                    let sample = f32::from_le_bytes([
                        buf.data[offset],
                        buf.data[offset + 1],
                        buf.data[offset + 2],
                        buf.data[offset + 3],
                    ]);
                    mono.push(sample);
                }
            }
            Ok(mono)
        }
        SampleFormat::I16 => {
            let total_values = buf.data.len() / 2;
            let num_samples = total_values / channels;
            let mut mono = Vec::with_capacity(num_samples);
            for i in 0..num_samples {
                let offset = i * channels * 2;
                if offset + 2 <= buf.data.len() {
                    let sample = i16::from_le_bytes([buf.data[offset], buf.data[offset + 1]]);
                    mono.push(sample as f32 / 32768.0);
                }
            }
            Ok(mono)
        }
        _ => Err(TarangError::AiError(format!(
            "unsupported sample format for mono extraction: {:?}",
            buf.sample_format
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::Duration;

    fn make_audio(format: SampleFormat, channels: u16, data: Vec<u8>) -> AudioBuffer {
        AudioBuffer {
            data: Bytes::from(data),
            sample_format: format,
            channels,
            sample_rate: 16000,
            num_samples: 0, // not used by extract_mono_f32
            timestamp: Duration::ZERO,
        }
    }

    #[test]
    fn f32_mono_passthrough() {
        let samples: Vec<f32> = vec![0.5, -0.25, 1.0];
        let mut data = Vec::new();
        for s in &samples {
            data.extend_from_slice(&s.to_le_bytes());
        }
        let buf = make_audio(SampleFormat::F32, 1, data);
        let mono = extract_mono_f32(&buf).unwrap();
        assert_eq!(mono.len(), 3);
        assert_eq!(mono[0], 0.5);
        assert_eq!(mono[1], -0.25);
        assert_eq!(mono[2], 1.0);
    }

    #[test]
    fn f32_stereo_extracts_first_channel() {
        // Interleaved stereo: [L0, R0, L1, R1]
        let interleaved: Vec<f32> = vec![0.5, -0.5, 0.25, -0.25];
        let mut data = Vec::new();
        for s in &interleaved {
            data.extend_from_slice(&s.to_le_bytes());
        }
        let buf = make_audio(SampleFormat::F32, 2, data);
        let mono = extract_mono_f32(&buf).unwrap();
        assert_eq!(mono.len(), 2);
        assert_eq!(mono[0], 0.5); // L0
        assert_eq!(mono[1], 0.25); // L1
    }

    #[test]
    fn i16_mono_conversion() {
        // i16 max (32767) -> ~0.99997
        // i16 0 -> 0.0
        // i16 -32768 -> -1.0
        let samples: Vec<i16> = vec![0, 32767, -32768];
        let mut data = Vec::new();
        for s in &samples {
            data.extend_from_slice(&s.to_le_bytes());
        }
        let buf = make_audio(SampleFormat::I16, 1, data);
        let mono = extract_mono_f32(&buf).unwrap();
        assert_eq!(mono.len(), 3);
        assert_eq!(mono[0], 0.0);
        assert!((mono[1] - 32767.0 / 32768.0).abs() < 1e-5);
        assert_eq!(mono[2], -1.0);
    }

    #[test]
    fn i16_stereo_extracts_first_channel() {
        // Interleaved: [L0, R0, L1, R1]
        let interleaved: Vec<i16> = vec![16384, -16384, 8192, -8192];
        let mut data = Vec::new();
        for s in &interleaved {
            data.extend_from_slice(&s.to_le_bytes());
        }
        let buf = make_audio(SampleFormat::I16, 2, data);
        let mono = extract_mono_f32(&buf).unwrap();
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 16384.0 / 32768.0).abs() < 1e-5);
        assert!((mono[1] - 8192.0 / 32768.0).abs() < 1e-5);
    }

    #[test]
    fn unsupported_format_returns_error() {
        let buf = make_audio(SampleFormat::F64, 1, vec![0; 16]);
        let result = extract_mono_f32(&buf);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("unsupported sample format"));
    }
}
