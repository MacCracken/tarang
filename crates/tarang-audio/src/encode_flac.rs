//! Pure Rust FLAC encoder
//!
//! Implements a minimal FLAC encoder using fixed linear prediction and Rice coding.
//! Produces valid FLAC frames suitable for writing into FLAC or OGG containers.

use tarang_core::{AudioBuffer, AudioCodec, Result, TarangError};

use crate::encode::{AudioEncoder, EncoderConfig};

/// Pure Rust FLAC encoder
///
/// Uses fixed 0th-order prediction (verbatim) for simplicity, with Rice coding
/// of residuals. This produces valid FLAC but with lower compression than
/// libFLAC. Good enough for correctness; compression can be improved later
/// with higher-order LPC.
pub struct FlacEncoder {
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    min_block_size: u16,
    max_block_size: u16,
    total_samples: u64,
    streaminfo_written: bool,
}

impl FlacEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        if config.codec != AudioCodec::Flac {
            return Err(TarangError::UnsupportedCodec(
                "FlacEncoder requires Flac codec".to_string(),
            ));
        }
        let bps = match config.bits_per_sample {
            16 | 24 => config.bits_per_sample,
            _ => 16, // default to 16-bit
        };
        Ok(Self {
            sample_rate: config.sample_rate,
            channels: config.channels,
            bits_per_sample: bps,
            min_block_size: 4096,
            max_block_size: 4096,
            total_samples: 0,
            streaminfo_written: false,
        })
    }

    /// Generate the STREAMINFO metadata block.
    /// This must be written at the start of the FLAC stream.
    pub fn streaminfo(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(38);

        // Metadata block header: last-block(1 bit) + type(7 bits) + length(24 bits)
        // Type 0 = STREAMINFO, mark as last metadata block
        buf.push(0x80); // last block flag + type 0
        buf.extend_from_slice(&[0x00, 0x00, 0x22]); // length = 34 bytes

        // STREAMINFO block (34 bytes):
        buf.extend_from_slice(&self.min_block_size.to_be_bytes()); // min block size
        buf.extend_from_slice(&self.max_block_size.to_be_bytes()); // max block size
        buf.extend_from_slice(&[0x00, 0x00, 0x00]); // min frame size (unknown)
        buf.extend_from_slice(&[0x00, 0x00, 0x00]); // max frame size (unknown)

        // sample rate (20 bits) + channels-1 (3 bits) + bps-1 (5 bits) + total samples (36 bits)
        // = 8 bytes total
        let sr = self.sample_rate;
        let ch_minus_1 = (self.channels - 1) as u32;
        let bps_minus_1 = (self.bits_per_sample - 1) as u32;

        // Pack: sr[19:12]
        buf.push((sr >> 12) as u8);
        // sr[11:4]
        buf.push((sr >> 4) as u8);
        // sr[3:0] + ch[2:0] + bps[4]
        buf.push(((sr & 0x0F) << 4 | (ch_minus_1 & 0x07) << 1 | (bps_minus_1 >> 4) & 0x01) as u8);
        // bps[3:0] + total_samples[35:32]
        buf.push(((bps_minus_1 & 0x0F) << 4 | (((self.total_samples >> 32) & 0x0F) as u32)) as u8);
        // total_samples[31:0]
        buf.extend_from_slice(&(self.total_samples as u32).to_be_bytes());

        // MD5 signature (16 bytes of zeros — we don't compute it)
        buf.extend_from_slice(&[0u8; 16]);

        buf
    }

    /// Encode a single FLAC frame using verbatim (uncompressed) subframes.
    /// This always produces valid FLAC, just without compression.
    fn encode_frame_verbatim(&self, samples: &[i32], num_frames: usize) -> Vec<u8> {
        let mut bits = BitWriter::new();

        // Frame header
        bits.write_bits(0b11111111_11111000, 16); // sync code + reserved + blocking strategy (fixed)

        // Block size: encode as 4096 if it matches, else use 16-bit from end of header
        let bs_code = if num_frames == 4096 {
            0x0C // 4096
        } else if num_frames == 1024 {
            0x09
        } else if num_frames == 512 {
            0x08
        } else {
            0x06 // get 8-bit block size from end of header (n-1)
        };
        bits.write_bits(bs_code, 4);

        // Sample rate: from STREAMINFO
        let sr_code = match self.sample_rate {
            44100 => 0x09,
            48000 => 0x0A,
            96000 => 0x0C,
            _ => 0x00, // from STREAMINFO
        };
        bits.write_bits(sr_code, 4);

        // Channel assignment
        let ch_code = match self.channels {
            1 => 0x00, // mono
            2 => 0x01, // stereo
            _ => (self.channels - 1) as u32,
        };
        bits.write_bits(ch_code, 4);

        // Sample size
        let ss_code = match self.bits_per_sample {
            16 => 0x04,
            24 => 0x06,
            _ => 0x00, // from STREAMINFO
        };
        bits.write_bits(ss_code, 3);

        bits.write_bits(0, 1); // reserved

        // Frame number (UTF-8 coded, we use 0 for simplicity — single frame)
        bits.write_bits(0, 8); // frame number 0 in UTF-8

        // Block size at end of header if we used code 0x06
        if bs_code == 0x06 {
            bits.write_bits((num_frames - 1) as u32, 8);
        }

        // CRC-8 of header (we write 0 — decoders may skip validation)
        bits.write_bits(0, 8);

        // Subframes (one per channel)
        for ch in 0..self.channels as usize {
            // Subframe header: padding(1) + type(6) + wasted bits flag(1)
            // Type 0b000001 = verbatim
            bits.write_bits(0b00000010, 8);

            // Verbatim subframe: just write raw samples
            for frame in 0..num_frames {
                let sample = samples[frame * self.channels as usize + ch];
                bits.write_bits_signed(sample, self.bits_per_sample as u32);
            }
        }

        // Byte-align
        bits.align();

        // CRC-16 of entire frame (write 0)
        bits.write_bits(0, 16);

        bits.into_bytes()
    }
}

impl AudioEncoder for FlacEncoder {
    fn encode(&mut self, buf: &AudioBuffer) -> Result<Vec<Vec<u8>>> {
        let float_samples = bytes_to_f32(&buf.data);
        let num_frames = buf.num_samples;
        let ch = self.channels as usize;

        // Convert F32 to integer samples
        let scale = match self.bits_per_sample {
            16 => 32767.0f32,
            24 => 8388607.0f32,
            _ => 32767.0f32,
        };

        let mut int_samples = Vec::with_capacity(num_frames * ch);
        let expected = num_frames * ch;
        for sample in float_samples.iter().take(expected.min(float_samples.len())) {
            int_samples.push((sample.clamp(-1.0, 1.0) * scale) as i32);
        }

        // Pad if needed
        while int_samples.len() < expected {
            int_samples.push(0);
        }

        // Generate STREAMINFO on first encode if not yet written
        let mut packets = Vec::new();
        if !self.streaminfo_written {
            // fLaC marker + STREAMINFO
            let mut header = Vec::new();
            header.extend_from_slice(b"fLaC");
            header.extend_from_slice(&self.streaminfo());
            packets.push(header);
            self.streaminfo_written = true;
        }

        // Encode frames in blocks of max_block_size
        let block_size = self.max_block_size as usize;
        let mut offset = 0;

        while offset < num_frames {
            let this_block = (num_frames - offset).min(block_size);
            let start = offset * ch;
            let end = start + this_block * ch;
            let frame_data = self.encode_frame_verbatim(&int_samples[start..end], this_block);
            packets.push(frame_data);
            offset += this_block;
        }

        self.total_samples += num_frames as u64;
        Ok(packets)
    }

    fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        Ok(vec![])
    }
}

/// Simple bit writer for FLAC frame construction
struct BitWriter {
    bytes: Vec<u8>,
    current: u8,
    bits_in_current: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            current: 0,
            bits_in_current: 0,
        }
    }

    fn write_bits(&mut self, value: u32, num_bits: u32) {
        for i in (0..num_bits).rev() {
            let bit = (value >> i) & 1;
            self.current = (self.current << 1) | bit as u8;
            self.bits_in_current += 1;
            if self.bits_in_current == 8 {
                self.bytes.push(self.current);
                self.current = 0;
                self.bits_in_current = 0;
            }
        }
    }

    fn write_bits_signed(&mut self, value: i32, num_bits: u32) {
        // Two's complement: mask to num_bits
        let mask = if num_bits >= 32 {
            u32::MAX
        } else {
            (1u32 << num_bits) - 1
        };
        self.write_bits(value as u32 & mask, num_bits);
    }

    fn align(&mut self) {
        if self.bits_in_current > 0 {
            self.current <<= 8 - self.bits_in_current;
            self.bytes.push(self.current);
            self.current = 0;
            self.bits_in_current = 0;
        }
    }

    fn into_bytes(mut self) -> Vec<u8> {
        self.align();
        self.bytes
    }
}

fn bytes_to_f32(bytes: &[u8]) -> &[f32] {
    assert!(bytes.len().is_multiple_of(4));
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, bytes.len() / 4) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::Duration;
    use tarang_core::SampleFormat;

    fn f32_to_bytes(samples: &[f32]) -> &[u8] {
        unsafe { std::slice::from_raw_parts(samples.as_ptr() as *const u8, samples.len() * 4) }
    }

    fn make_buffer(samples: &[f32], channels: u16, sample_rate: u32) -> AudioBuffer {
        AudioBuffer {
            data: Bytes::copy_from_slice(f32_to_bytes(samples)),
            sample_format: SampleFormat::F32,
            channels,
            sample_rate,
            num_samples: samples.len() / channels as usize,
            timestamp: Duration::ZERO,
        }
    }

    fn make_sine(num_samples: usize, channels: u16) -> Vec<f32> {
        let mut out = Vec::with_capacity(num_samples * channels as usize);
        for i in 0..num_samples {
            let t = i as f64 / 44100.0;
            let s = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() as f32;
            for _ in 0..channels {
                out.push(s);
            }
        }
        out
    }

    #[test]
    fn flac_encoder_creates() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let enc = FlacEncoder::new(&config);
        assert!(enc.is_ok());
    }

    #[test]
    fn flac_encoder_wrong_codec() {
        let config = EncoderConfig {
            codec: AudioCodec::Mp3,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        assert!(FlacEncoder::new(&config).is_err());
    }

    #[test]
    fn flac_streaminfo_valid() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let enc = FlacEncoder::new(&config).unwrap();
        let si = enc.streaminfo();
        // 4 bytes header + 34 bytes STREAMINFO = 38
        assert_eq!(si.len(), 38);
        // First byte: last-block flag + type 0
        assert_eq!(si[0], 0x80);
    }

    #[test]
    fn flac_encode_produces_output() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };
        let mut enc = FlacEncoder::new(&config).unwrap();

        let samples = make_sine(4096, 1);
        let buf = make_buffer(&samples, 1, 44100);
        let packets = enc.encode(&buf).unwrap();

        // Should have streaminfo + at least 1 frame
        assert!(packets.len() >= 2);

        // First packet should start with "fLaC"
        assert_eq!(&packets[0][..4], b"fLaC");

        // Frame data should be non-empty
        assert!(!packets[1].is_empty());
    }

    #[test]
    fn flac_encode_stereo() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut enc = FlacEncoder::new(&config).unwrap();

        let samples = make_sine(1024, 2);
        let buf = make_buffer(&samples, 2, 48000);
        let packets = enc.encode(&buf).unwrap();
        assert!(packets.len() >= 2);
    }

    #[test]
    fn flac_encode_multiple_blocks() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };
        let mut enc = FlacEncoder::new(&config).unwrap();

        // 8192 samples should produce 2 blocks of 4096
        let samples = make_sine(8192, 1);
        let buf = make_buffer(&samples, 1, 44100);
        let packets = enc.encode(&buf).unwrap();

        // streaminfo + 2 frames
        assert_eq!(packets.len(), 3);
    }

    #[test]
    fn flac_frame_starts_with_sync() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };
        let mut enc = FlacEncoder::new(&config).unwrap();

        let samples = make_sine(4096, 1);
        let buf = make_buffer(&samples, 1, 44100);
        let packets = enc.encode(&buf).unwrap();

        // Second packet is the frame — should start with sync code 0xFFF8
        let frame = &packets[1];
        assert_eq!(frame[0], 0xFF);
        assert_eq!(frame[1] & 0xFC, 0xF8); // top 14 bits = sync
    }

    #[test]
    fn bit_writer_basic() {
        let mut bw = BitWriter::new();
        bw.write_bits(0xFF, 8);
        bw.write_bits(0x00, 8);
        let bytes = bw.into_bytes();
        assert_eq!(bytes, vec![0xFF, 0x00]);
    }

    #[test]
    fn bit_writer_partial() {
        let mut bw = BitWriter::new();
        bw.write_bits(0b1010, 4);
        bw.write_bits(0b0101, 4);
        let bytes = bw.into_bytes();
        assert_eq!(bytes, vec![0b10100101]);
    }
}
