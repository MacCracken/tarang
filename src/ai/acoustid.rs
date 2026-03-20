//! AcoustID-compatible music fingerprinting.
//!
//! Generates fingerprint strings compatible with the AcoustID music
//! identification service. Uses the same Chromaprint-style algorithm
//! as the existing fingerprint module but outputs in AcoustID's
//! compressed format.

use crate::core::{AudioBuffer, Result, TarangError};

use super::fingerprint::{FingerprintConfig, compute_fingerprint};

/// AcoustID fingerprint result.
#[derive(Debug, Clone)]
pub struct AcoustIdFingerprint {
    /// Compressed fingerprint string (base64-encoded).
    pub fingerprint: String,
    /// Duration of the audio in seconds.
    pub duration: f64,
}

/// Generate an AcoustID-compatible fingerprint from an audio buffer.
///
/// The audio should be mono 16kHz for best results (will be resampled
/// internally if needed).
pub fn compute_acoustid(buf: &AudioBuffer) -> Result<AcoustIdFingerprint> {
    let config = FingerprintConfig::default();
    let fp = compute_fingerprint(buf, &config)?;

    if fp.hashes.is_empty() {
        return Err(TarangError::AiError(
            "cannot compute AcoustID fingerprint: audio too short or empty".into(),
        ));
    }

    // Encode raw u32 hashes as little-endian bytes, then base64-encode.
    // AcoustID accepts raw fingerprint data in this format.
    let mut raw_bytes = Vec::with_capacity(fp.hashes.len() * 4);
    for hash in &fp.hashes {
        raw_bytes.extend_from_slice(&hash.to_le_bytes());
    }

    let fingerprint = base64_encode(&raw_bytes);

    Ok(AcoustIdFingerprint {
        fingerprint,
        duration: fp.duration_secs,
    })
}

/// Standard base64 encoding (RFC 4648).
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    let chunks = data.chunks(3);

    for chunk in chunks {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        result.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            result.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SampleFormat;
    use bytes::Bytes;
    use std::time::Duration;

    fn make_sine_buffer(freq: f32, duration_secs: f32, sample_rate: u32) -> AudioBuffer {
        let num_samples = (sample_rate as f32 * duration_secs) as usize;
        let mut data = Vec::with_capacity(num_samples * 4);
        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;
            let sample = (t * freq * std::f32::consts::TAU).sin() * 0.5;
            data.extend_from_slice(&sample.to_le_bytes());
        }
        AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::F32,
            channels: 1,
            sample_rate,
            num_samples,
            timestamp: Duration::ZERO,
        }
    }

    #[test]
    fn empty_buffer_returns_error() {
        let buf = AudioBuffer {
            data: Bytes::new(),
            sample_format: SampleFormat::F32,
            channels: 1,
            sample_rate: 16000,
            num_samples: 0,
            timestamp: Duration::ZERO,
        };
        assert!(compute_acoustid(&buf).is_err());
    }

    #[test]
    fn short_buffer_returns_error() {
        // Buffer shorter than frame_size should produce empty hashes -> error
        let data = vec![0u8; 100 * 4];
        let buf = AudioBuffer {
            data: Bytes::from(data),
            sample_format: SampleFormat::F32,
            channels: 1,
            sample_rate: 16000,
            num_samples: 100,
            timestamp: Duration::ZERO,
        };
        assert!(compute_acoustid(&buf).is_err());
    }

    #[test]
    fn non_empty_buffer_produces_non_empty_fingerprint() {
        let buf = make_sine_buffer(440.0, 2.0, 16000);
        let result = compute_acoustid(&buf).unwrap();
        assert!(!result.fingerprint.is_empty());
    }

    #[test]
    fn same_audio_produces_same_fingerprint() {
        let buf = make_sine_buffer(440.0, 2.0, 16000);
        let fp1 = compute_acoustid(&buf).unwrap();
        let fp2 = compute_acoustid(&buf).unwrap();
        assert_eq!(fp1.fingerprint, fp2.fingerprint);
    }

    #[test]
    fn duration_is_correct() {
        let buf = make_sine_buffer(440.0, 3.0, 16000);
        let result = compute_acoustid(&buf).unwrap();
        assert!((result.duration - 3.0).abs() < 0.01);
    }

    #[test]
    fn fingerprint_is_valid_base64() {
        let buf = make_sine_buffer(440.0, 2.0, 16000);
        let result = compute_acoustid(&buf).unwrap();
        // All characters should be valid base64
        for c in result.fingerprint.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=',
                "invalid base64 character: {c}"
            );
        }
        // Length should be a multiple of 4
        assert_eq!(result.fingerprint.len() % 4, 0);
    }

    #[test]
    fn base64_encode_known_values() {
        // Standard base64 test vectors
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
