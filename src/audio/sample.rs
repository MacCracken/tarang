//! Shared sample format conversion utilities
//!
//! Safe wrappers for reinterpreting `&[u8]` ↔ `&[f32]` byte buffers.
//! All audio modules use these instead of duplicating unsafe pointer casts.
//! Also provides PCM scaling constants for integer↔float conversion.

/// Maximum value for signed 16-bit PCM (used for f32 ↔ i16 conversion)
pub(crate) const I16_SCALE: f32 = 32767.0;

/// Maximum value for signed 24-bit PCM (used for f32 ↔ i24 conversion)
pub(crate) const I24_SCALE: f32 = 8388607.0;

/// Maximum value for signed 32-bit PCM (used for f32 ↔ i32 conversion)
pub(crate) const I32_SCALE: f32 = 2147483647.0;

/// Reinterpret a byte slice as f32 samples.
///
/// Returns an empty slice if the input length is not a multiple of 4.
/// The input must originate from an F32 audio buffer (heap-allocated,
/// so alignment is guaranteed by the allocator on all common platforms).
pub(crate) fn bytes_to_f32(bytes: &[u8]) -> &[f32] {
    let len = bytes.len() / 4;
    if len == 0 || !bytes.len().is_multiple_of(4) {
        return &[];
    }
    debug_assert!(
        bytes.as_ptr().align_offset(std::mem::align_of::<f32>()) == 0,
        "byte buffer not aligned for f32"
    );
    // Safety: AudioBuffer data originates from Vec<f32> serialized via to_le_bytes or
    // Bytes::copy_from_slice, so alignment is guaranteed by the heap allocator (>=8 bytes).
    // Length is validated above as a multiple of 4.
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, len) }
}

/// Reinterpret an f32 sample slice as raw bytes.
///
/// This is always safe — f32 has no invalid bit patterns.
pub(crate) fn f32_to_bytes(samples: &[f32]) -> &[u8] {
    let byte_len = samples.len().checked_mul(4).unwrap_or(0);
    if byte_len == 0 {
        return &[];
    }
    // Safety: f32 is Pod-like — every bit pattern is valid when read as u8.
    // byte_len is validated above via checked_mul.
    unsafe { std::slice::from_raw_parts(samples.as_ptr() as *const u8, byte_len) }
}

/// Create an `AudioBuffer` from f32 samples (test utility, available to all audio test modules).
#[cfg(test)]
pub(crate) fn make_test_buffer(
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
) -> crate::core::AudioBuffer {
    crate::core::AudioBuffer {
        data: bytes::Bytes::copy_from_slice(f32_to_bytes(samples)),
        sample_format: crate::core::SampleFormat::F32,
        channels,
        sample_rate,
        num_samples: samples.len() / channels as usize,
        timestamp: std::time::Duration::ZERO,
    }
}

/// Generate a sine wave as interleaved f32 samples (test utility).
#[cfg(test)]
pub(crate) fn make_test_sine(
    freq: f64,
    sample_rate: u32,
    num_samples: usize,
    channels: u16,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(num_samples * channels as usize);
    for i in 0..num_samples {
        let t = i as f64 / sample_rate as f64;
        let s = (t * freq * 2.0 * std::f64::consts::PI).sin() as f32;
        for _ in 0..channels {
            out.push(s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let original = [0.5f32, -0.25, 1.0, 0.0];
        let bytes = f32_to_bytes(&original);
        let back = bytes_to_f32(bytes);
        assert_eq!(back, &original);
    }

    #[test]
    fn empty_input() {
        assert!(bytes_to_f32(&[]).is_empty());
        assert!(f32_to_bytes(&[]).is_empty());
    }

    #[test]
    fn not_multiple_of_four() {
        assert!(bytes_to_f32(&[1, 2, 3]).is_empty());
        assert!(bytes_to_f32(&[1, 2, 3, 4, 5]).is_empty());
    }

    #[test]
    fn single_sample() {
        let val = [0.42f32];
        let bytes = f32_to_bytes(&val);
        assert_eq!(bytes.len(), 4);
        let back = bytes_to_f32(bytes);
        assert_eq!(back, &val);
    }

    #[test]
    fn byte_length() {
        let samples = [1.0f32; 100];
        assert_eq!(f32_to_bytes(&samples).len(), 400);
    }
}
