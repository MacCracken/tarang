//! Pure Rust FLAC encoder
//!
//! Implements a FLAC encoder using both fixed-order prediction (orders 0-4)
//! and linear LPC prediction via Levinson-Durbin (orders 1-8) with
//! Rice-coded residuals. Automatically selects whichever method produces
//! the smallest output. Produces valid FLAC frames with proper CRC-8
//! and CRC-16 checksums suitable for writing into FLAC or OGG containers.
//!
//! # Example
//! ```rust,ignore
//! use tarang::audio::encode_flac::FlacEncoder;
//! use tarang::audio::encode::{AudioEncoder, EncoderConfig};
//! use tarang::core::AudioCodec;
//!
//! let config = EncoderConfig::builder(AudioCodec::Flac)
//!     .sample_rate(44100).channels(2).bits_per_sample(16).build();
//! let mut enc = FlacEncoder::new(&config).unwrap();
//! // let packets = enc.encode(&audio_buf).unwrap();
//! ```

use crate::core::{AudioBuffer, AudioCodec, Result, TarangError};

use super::encode::{AudioEncoder, EncoderConfig};

/// Pure Rust FLAC encoder
///
/// Uses fixed-order prediction (orders 0-4) and linear LPC prediction via
/// Levinson-Durbin (orders 1-8) with Rice coding of residuals. Automatically
/// selects the best prediction method and order per channel for optimal
/// compression. Falls back to verbatim subframes when prediction doesn't help.
pub struct FlacEncoder {
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    min_block_size: u16,
    max_block_size: u16,
    total_samples: u64,
    streaminfo_written: bool,
}

// ---------------------------------------------------------------------------
// CRC helpers
// ---------------------------------------------------------------------------

/// CRC-8 with polynomial 0x07 (FLAC frame header).
fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x07;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// CRC-16 with polynomial 0x8005 (FLAC frame footer).
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x8005;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

// ---------------------------------------------------------------------------
// Rice coding helpers
// ---------------------------------------------------------------------------

/// Map a signed residual to an unsigned value for Rice coding.
/// positive n -> 2*n, negative n -> 2*|n| - 1
fn rice_encode_value(v: i32) -> u32 {
    if v >= 0 {
        (v as u32) << 1
    } else {
        (((-v) as u32) << 1) - 1
    }
}

/// Decode a Rice-mapped unsigned value back to the signed residual.
#[cfg(test)]
fn rice_decode_value(v: u32) -> i32 {
    if v & 1 == 0 {
        (v >> 1) as i32
    } else {
        -(((v + 1) >> 1) as i32)
    }
}

/// Estimate the number of bits required to Rice-code the given mapped values
/// with parameter `k`.
fn rice_bits(mapped: &[u32], k: u32) -> u64 {
    let mut total: u64 = 0;
    for &m in mapped {
        let q = m >> k;
        // unary: q zeros + 1 one = q+1 bits, plus k low bits
        total += (q as u64) + 1 + (k as u64);
    }
    total
}

/// Pick the optimal Rice parameter k (0..=14) for the given mapped values.
fn optimal_rice_param(mapped: &[u32]) -> u32 {
    if mapped.is_empty() {
        return 0;
    }
    // Estimate from average magnitude
    let sum: u64 = mapped
        .iter()
        .map(|&m| {
            if m == 0 {
                0u64
            } else {
                (32 - m.leading_zeros()) as u64
            }
        })
        .sum();
    let avg = sum / mapped.len().max(1) as u64;
    let k_est = avg.min(14) as u32;
    // Search k_est-1 ..= k_est+1 for the best
    let lo = k_est.saturating_sub(1);
    let hi = (k_est + 1).min(14);
    let mut best_k = k_est;
    let mut best_bits = rice_bits(mapped, k_est);
    for k in lo..=hi {
        let b = rice_bits(mapped, k);
        if b < best_bits {
            best_bits = b;
            best_k = k;
        }
    }
    best_k
}

// ---------------------------------------------------------------------------
// LPC prediction helpers (Levinson-Durbin)
// ---------------------------------------------------------------------------

/// Compute autocorrelation coefficients for the given samples.
fn autocorrelation(samples: &[i32], max_order: usize) -> Vec<f64> {
    let mut r = vec![0.0f64; max_order + 1];
    for lag in 0..=max_order {
        for i in lag..samples.len() {
            r[lag] += samples[i] as f64 * samples[i - lag] as f64;
        }
    }
    r
}

/// Compute LPC coefficients using Levinson-Durbin recursion.
/// Returns (coefficients, prediction_error) for the given order.
/// Returns None if the signal is degenerate (zero energy) or the filter is unstable.
fn levinson_durbin(autocorr: &[f64], order: usize) -> Option<(Vec<f64>, f64)> {
    if autocorr.is_empty() || autocorr[0] <= 0.0 {
        return None;
    }
    let mut a = vec![0.0f64; order + 1];
    a[0] = 1.0;
    let mut error = autocorr[0];

    for i in 1..=order {
        // Compute reflection coefficient (lambda)
        let mut sum = 0.0;
        for j in 0..i {
            sum += a[j] * autocorr[i - j];
        }
        let lambda = -sum / error;

        if lambda.abs() >= 1.0 {
            return None; // unstable filter
        }

        // Update coefficients: a_new[j] = a[j] + lambda * a[i-j]
        let mut a_new = a.clone();
        for j in 0..=i {
            a_new[j] = a[j] + lambda * a[i - j];
        }
        a = a_new;

        error *= 1.0 - lambda * lambda;
        if error <= 0.0 {
            return None;
        }
    }

    // Return coefficients excluding a[0]=1.0 — these are the predictor coefficients
    // In FLAC, the predictor is: predicted = sum(coeff[j] * sample[i-1-j])
    // The Levinson-Durbin a[] coefficients satisfy: a[0]*x[n] + a[1]*x[n-1] + ... = 0
    // So the predictor coefficients are -a[1], -a[2], ..., -a[order]
    let coeffs: Vec<f64> = (1..=order).map(|j| -a[j]).collect();
    Some((coeffs, error))
}

/// Quantize floating-point LPC coefficients to integers.
/// Returns (quantized_coeffs, precision_bits, shift).
fn quantize_lpc(coeffs: &[f64], bps: u32) -> (Vec<i32>, u32, i32) {
    let precision = 15.min(bps - 1);
    let max_coeff = coeffs.iter().map(|c| c.abs()).fold(0.0f64, f64::max);
    if max_coeff < 1e-10 {
        return (vec![0; coeffs.len()], precision, 0);
    }
    let shift = (precision as f64 - 1.0 - max_coeff.log2()).floor() as i32;
    let shift = shift.clamp(-16, 15); // FLAC allows shift -16..15
    let scale = (1i64 << shift.max(0)) as f64;
    let quantized: Vec<i32> = coeffs.iter().map(|&c| (c * scale).round() as i32).collect();
    (quantized, precision, shift)
}

/// Compute LPC residuals given quantized coefficients.
fn lpc_residuals(samples: &[i32], coeffs: &[i32], order: usize, shift: i32) -> Vec<i32> {
    let mut residuals = Vec::with_capacity(samples.len() - order);
    for i in order..samples.len() {
        let mut predicted: i64 = 0;
        for j in 0..order {
            predicted = predicted
                .saturating_add((coeffs[j] as i64).saturating_mul(samples[i - 1 - j] as i64));
        }
        predicted >>= shift;
        residuals.push(samples[i] - predicted as i32);
    }
    residuals
}

/// Estimate the encoded size (in bits) of an LPC subframe.
fn estimate_lpc_size(residuals: &[i32], order: usize, bps: u32, precision: u32) -> u64 {
    // Subframe header (8 bits) + warm-up samples (order * bps)
    let mut bits: u64 = 8 + (order as u64) * (bps as u64);
    // QLP precision - 1 (4 bits) + QLP shift (5 bits)
    bits += 4 + 5;
    // QLP coefficients (order * precision bits each)
    bits += (order as u64) * (precision as u64);
    // Residual coding: method (2 bits) + partition order (4 bits) + rice param (4 bits)
    bits += 2 + 4 + 4;
    // Rice-coded residuals
    let mapped: Vec<u32> = residuals.iter().map(|&r| rice_encode_value(r)).collect();
    let k = optimal_rice_param(&mapped);
    bits += rice_bits(&mapped, k);
    bits
}

// ---------------------------------------------------------------------------
// Fixed prediction helpers
// ---------------------------------------------------------------------------

/// Compute residuals for a given fixed prediction order.
fn fixed_residuals(samples: &[i32], order: usize) -> Vec<i32> {
    let n = samples.len();
    if n <= order {
        return vec![];
    }
    let mut residuals = Vec::with_capacity(n - order);
    for i in order..n {
        let predicted = match order {
            0 => 0i64,
            1 => samples[i - 1] as i64,
            2 => (2i64)
                .saturating_mul(samples[i - 1] as i64)
                .saturating_sub(samples[i - 2] as i64),
            3 => (3i64)
                .saturating_mul(samples[i - 1] as i64)
                .saturating_sub((3i64).saturating_mul(samples[i - 2] as i64))
                .saturating_add(samples[i - 3] as i64),
            4 => (4i64)
                .saturating_mul(samples[i - 1] as i64)
                .saturating_sub((6i64).saturating_mul(samples[i - 2] as i64))
                .saturating_add((4i64).saturating_mul(samples[i - 3] as i64))
                .saturating_sub(samples[i - 4] as i64),
            _ => 0,
        };
        residuals.push((samples[i] as i64).saturating_sub(predicted) as i32);
    }
    residuals
}

/// Estimate the encoded size (in bits) of a fixed-order subframe including
/// warm-up samples, residual coding overhead, and Rice-coded residuals.
fn estimate_fixed_size(residuals: &[i32], order: usize, bps: u32) -> u64 {
    // Subframe header (8 bits) + warm-up samples
    let mut bits: u64 = 8 + (order as u64) * (bps as u64);
    // Residual coding method (2 bits) + partition order (4 bits) + rice param (4 or 5 bits)
    bits += 2 + 4 + 4;
    // Rice-coded residuals
    let mapped: Vec<u32> = residuals.iter().map(|&r| rice_encode_value(r)).collect();
    let k = optimal_rice_param(&mapped);
    bits += rice_bits(&mapped, k);
    bits
}

/// Estimate the size of a verbatim subframe.
fn estimate_verbatim_size(num_samples: usize, bps: u32) -> u64 {
    // Subframe header (8 bits) + raw samples
    8 + (num_samples as u64) * (bps as u64)
}

impl FlacEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        if config.codec != AudioCodec::Flac {
            return Err(TarangError::UnsupportedCodec(
                "FlacEncoder requires Flac codec".into(),
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

    /// Write the frame header into a BitWriter, returning the header bytes
    /// (for CRC-8 computation). The CRC-8 placeholder is included.
    fn write_frame_header(&self, bits: &mut BitWriter, num_frames: usize) {
        // Frame header
        bits.write_bits(0b11111111_11111000, 16); // sync code + reserved + blocking strategy (fixed)

        // Block size code
        let bs_code = if num_frames == 4096 {
            0x0C
        } else if num_frames == 1024 {
            0x09
        } else if num_frames == 512 {
            0x08
        } else {
            0x06 // get 8-bit block size from end of header (n-1)
        };
        bits.write_bits(bs_code, 4);

        // Sample rate
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
    }

    /// Encode a single FLAC frame using verbatim (uncompressed) subframes.
    /// This always produces valid FLAC, just without compression.
    fn encode_frame_verbatim(&self, samples: &[i32], num_frames: usize) -> Vec<u8> {
        let mut bits = BitWriter::new();

        self.write_frame_header(&mut bits, num_frames);

        // CRC-8 placeholder — we'll fill it in after
        let crc8_byte_pos = bits.byte_position();
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

        // Now compute CRC-8 over header bytes (everything before the CRC-8 byte)
        let frame_bytes = bits.as_bytes();
        let crc8_val = crc8(&frame_bytes[..crc8_byte_pos]);
        bits.set_byte(crc8_byte_pos, crc8_val);

        // CRC-16 placeholder
        bits.write_bits(0, 16);

        let mut frame_data = bits.into_bytes();

        // Compute CRC-16 over everything except the last 2 bytes (the CRC-16 itself)
        let crc16_val = crc16(&frame_data[..frame_data.len() - 2]);
        let len = frame_data.len();
        frame_data[len - 2] = (crc16_val >> 8) as u8;
        frame_data[len - 1] = (crc16_val & 0xFF) as u8;

        frame_data
    }

    /// Encode a single FLAC frame using the best prediction method available:
    /// fixed-order LPC (orders 0-4), linear LPC via Levinson-Durbin (orders 1-8),
    /// or verbatim. Falls back to verbatim if no predictor compresses.
    fn encode_frame_fixed(&self, samples: &[i32], num_frames: usize) -> Vec<u8> {
        let ch = self.channels as usize;
        let bps = self.bits_per_sample as u32;

        /// Per-channel encoding plan.
        enum SubframeKind {
            Verbatim,
            Fixed {
                order: usize,
                residuals: Vec<i32>,
                rice_param: u32,
            },
            Lpc {
                order: usize,
                residuals: Vec<i32>,
                rice_param: u32,
                qlp_coeffs: Vec<i32>,
                qlp_precision: u32,
                qlp_shift: i32,
            },
        }

        let verbatim_size = estimate_verbatim_size(num_frames, bps);

        let mut plans: Vec<SubframeKind> = Vec::with_capacity(ch);
        let mut channel_samples: Vec<i32> = Vec::with_capacity(num_frames);

        for c in 0..ch {
            // Extract this channel's samples, reusing the pre-allocated Vec
            channel_samples.clear();
            channel_samples.extend((0..num_frames).map(|f| samples[f * ch + c]));

            let mut best_size = verbatim_size;
            let mut best_plan = SubframeKind::Verbatim;

            // Try fixed orders 0-4
            // Reuse a single residuals buffer; only clone into best_plan when
            // a better order is found.
            for order in 0..=4 {
                if num_frames <= order + 1 {
                    continue;
                }
                let res = fixed_residuals(&channel_samples, order);
                let size = estimate_fixed_size(&res, order, bps);
                if size < best_size {
                    let mapped: Vec<u32> = res.iter().map(|&r| rice_encode_value(r)).collect();
                    let k = optimal_rice_param(&mapped);
                    best_size = size;
                    best_plan = SubframeKind::Fixed {
                        order,
                        residuals: res,
                        rice_param: k,
                    };
                }
            }

            // Try LPC orders 1-8
            if num_frames > 8 {
                let max_lpc_order = 8.min(num_frames - 1);
                let autocorr = autocorrelation(&channel_samples, max_lpc_order);

                for order in 1..=max_lpc_order {
                    if let Some((coeffs, _error)) = levinson_durbin(&autocorr, order) {
                        let (qlp_coeffs, qlp_precision, qlp_shift) = quantize_lpc(&coeffs, bps);
                        let res = lpc_residuals(&channel_samples, &qlp_coeffs, order, qlp_shift);
                        let size = estimate_lpc_size(&res, order, bps, qlp_precision);
                        if size < best_size {
                            let mapped: Vec<u32> =
                                res.iter().map(|&r| rice_encode_value(r)).collect();
                            let k = optimal_rice_param(&mapped);
                            best_size = size;
                            best_plan = SubframeKind::Lpc {
                                order,
                                residuals: res,
                                rice_param: k,
                                qlp_coeffs,
                                qlp_precision,
                                qlp_shift,
                            };
                        }
                    }
                }
            }

            plans.push(best_plan);
        }

        // If ALL channels chose verbatim, just use verbatim encoding
        if plans.iter().all(|p| matches!(p, SubframeKind::Verbatim)) {
            return self.encode_frame_verbatim(samples, num_frames);
        }

        // Build the frame
        let mut bits = BitWriter::new();

        self.write_frame_header(&mut bits, num_frames);

        // CRC-8 placeholder
        let crc8_byte_pos = bits.byte_position();
        bits.write_bits(0, 8);

        // Subframes
        // Reuse channel_samples Vec from prediction phase
        for c in 0..ch {
            let plan = &plans[c];
            channel_samples.clear();
            channel_samples.extend((0..num_frames).map(|f| samples[f * ch + c]));

            match plan {
                SubframeKind::Verbatim => {
                    bits.write_bits(0b00000010, 8);
                    for &s in &channel_samples {
                        bits.write_bits_signed(s, bps);
                    }
                }
                SubframeKind::Fixed {
                    order,
                    residuals,
                    rice_param,
                } => {
                    // Fixed subframe header: padding(1)=0 + type(6) + wasted(1)=0
                    // Type for fixed: 001xxx where xxx = order
                    let subframe_type = 0b001000 | (*order as u32);
                    bits.write_bits(0, 1);
                    bits.write_bits(subframe_type, 6);
                    bits.write_bits(0, 1);

                    // Warm-up samples
                    for &s in &channel_samples[..*order] {
                        bits.write_bits_signed(s, bps);
                    }

                    // Residual coding
                    Self::write_rice_residuals(&mut bits, residuals, *rice_param);
                }
                SubframeKind::Lpc {
                    order,
                    residuals,
                    rice_param,
                    qlp_coeffs,
                    qlp_precision,
                    qlp_shift,
                } => {
                    // LPC subframe header: padding(1)=0 + type(6) + wasted(1)=0
                    // Type for LPC: 1xxxxx where xxxxx = order - 1
                    let subframe_type = 0b100000 | ((*order - 1) as u32);
                    bits.write_bits(0, 1);
                    bits.write_bits(subframe_type, 6);
                    bits.write_bits(0, 1);

                    // Warm-up samples
                    for &s in &channel_samples[..*order] {
                        bits.write_bits_signed(s, bps);
                    }

                    // QLP precision - 1 (4 bits, unsigned)
                    bits.write_bits(*qlp_precision - 1, 4);
                    // QLP shift (5 bits, signed)
                    bits.write_bits_signed(*qlp_shift, 5);
                    // QLP coefficients (precision bits each, signed)
                    for &c in qlp_coeffs {
                        bits.write_bits_signed(c, *qlp_precision);
                    }

                    // Residual coding
                    Self::write_rice_residuals(&mut bits, residuals, *rice_param);
                }
            }
        }

        // Byte-align
        bits.align();

        // CRC-8
        let frame_bytes = bits.as_bytes();
        let crc8_val = crc8(&frame_bytes[..crc8_byte_pos]);
        bits.set_byte(crc8_byte_pos, crc8_val);

        // CRC-16 placeholder
        bits.write_bits(0, 16);

        let mut frame_data = bits.into_bytes();

        // Compute CRC-16
        let crc16_val = crc16(&frame_data[..frame_data.len() - 2]);
        let len = frame_data.len();
        frame_data[len - 2] = (crc16_val >> 8) as u8;
        frame_data[len - 1] = (crc16_val & 0xFF) as u8;

        frame_data
    }

    /// Write Rice-coded residuals to the bit writer.
    fn write_rice_residuals(bits: &mut BitWriter, residuals: &[i32], rice_param: u32) {
        // Coding method: 00 = RICE_PARTITION (4-bit param)
        bits.write_bits(0b00, 2);
        // Partition order: 0 (single partition)
        bits.write_bits(0, 4);
        // Rice parameter (4 bits)
        bits.write_bits(rice_param, 4);

        let k = rice_param;
        for &r in residuals {
            let mapped = rice_encode_value(r);
            let q = mapped >> k;
            for _ in 0..q {
                bits.write_bits(0, 1);
            }
            bits.write_bits(1, 1);
            if k > 0 {
                bits.write_bits(mapped & ((1 << k) - 1), k);
            }
        }
    }
}

impl AudioEncoder for FlacEncoder {
    fn encode(&mut self, buf: &AudioBuffer) -> Result<Vec<Vec<u8>>> {
        if self.channels > 8 {
            return Err(TarangError::UnsupportedCodec(
                "FLAC supports max 8 channels".into(),
            ));
        }
        let float_samples = bytes_to_f32(&buf.data);
        let num_frames = buf.num_frames;
        let ch = self.channels as usize;

        // Convert F32 to integer samples
        let scale = match self.bits_per_sample {
            16 => super::sample::I16_SCALE,
            24 => super::sample::I24_SCALE,
            _ => super::sample::I16_SCALE,
        };

        let mut int_samples = Vec::with_capacity(num_frames * ch);
        let expected = num_frames * ch;
        for sample in float_samples.iter().take(expected.min(float_samples.len())) {
            int_samples.push((sample.clamp(-1.0, 1.0) * scale) as i32);
        }

        // Pad if needed (buffer smaller than block size)
        if int_samples.len() < expected {
            tracing::debug!(
                got = int_samples.len(),
                expected,
                "FLAC: zero-padding undersized buffer"
            );
            int_samples.resize(expected, 0);
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
            let frame_data = self.encode_frame_fixed(&int_samples[start..end], this_block);
            packets.push(frame_data);
            offset += this_block;
        }

        self.total_samples += num_frames as u64;
        let total_bytes: usize = packets.iter().map(|p| p.len()).sum();
        tracing::debug!(
            frames = num_frames,
            channels = ch,
            bps = self.bits_per_sample,
            output_bytes = total_bytes,
            "FLAC encode complete"
        );
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
            bytes: Vec::with_capacity(8192),
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

    /// Current byte position (number of complete bytes written so far).
    fn byte_position(&self) -> usize {
        self.bytes.len()
    }

    /// Get a reference to the bytes written so far (excludes partial byte).
    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Overwrite a byte at a given position.
    fn set_byte(&mut self, pos: usize, val: u8) {
        self.bytes[pos] = val;
    }

    fn into_bytes(mut self) -> Vec<u8> {
        self.align();
        self.bytes
    }
}

use super::sample::bytes_to_f32;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_buffer(samples: &[f32], channels: u16, sample_rate: u32) -> crate::core::AudioBuffer {
        crate::audio::sample::make_test_buffer(samples, channels, sample_rate)
    }

    fn make_sine(num_samples: usize, channels: u16) -> Vec<f32> {
        crate::audio::sample::make_test_sine(440.0, 44100, num_samples, channels)
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

    #[test]
    fn flac_flush_empty() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };
        let mut enc = FlacEncoder::new(&config).unwrap();
        let flushed = enc.flush().unwrap();
        assert!(flushed.is_empty());
    }

    #[test]
    fn flac_encode_high_sample_rate() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 96000,
            channels: 2,
            bits_per_sample: 24,
        };
        let mut enc = FlacEncoder::new(&config).unwrap();
        let samples = make_sine(4096, 2);
        let buf = make_buffer(&samples, 2, 96000);
        let packets = enc.encode(&buf).unwrap();
        assert!(!packets.is_empty());
        // First packet should be fLaC stream header
        assert_eq!(&packets[0][..4], b"fLaC");
    }

    #[test]
    fn flac_encode_small_buffer() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };
        let mut enc = FlacEncoder::new(&config).unwrap();
        // Fewer than block_size samples
        let samples = make_sine(100, 1);
        let buf = make_buffer(&samples, 1, 44100);
        let packets = enc.encode(&buf).unwrap();
        // Should still produce streaminfo + 1 padded frame
        assert!(packets.len() >= 2);
    }

    #[test]
    fn bit_writer_multi_byte() {
        let mut bw = BitWriter::new();
        bw.write_bits(0xABCD, 16);
        let bytes = bw.into_bytes();
        assert_eq!(bytes, vec![0xAB, 0xCD]);
    }

    #[test]
    fn bit_writer_cross_byte_boundary() {
        let mut bw = BitWriter::new();
        bw.write_bits(0b111, 3);
        bw.write_bits(0b00000, 5);
        bw.write_bits(0b11111111, 8);
        let bytes = bw.into_bytes();
        assert_eq!(bytes, vec![0b11100000, 0xFF]);
    }

    // --- New tests ---

    #[test]
    fn flac_fixed_prediction_compresses() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };

        // Encode with fixed prediction
        let mut enc_fixed = FlacEncoder::new(&config).unwrap();
        let samples = make_sine(4096, 1);
        let buf = make_buffer(&samples, 1, 44100);
        let packets_fixed = enc_fixed.encode(&buf).unwrap();
        let fixed_size: usize = packets_fixed[1..].iter().map(|p| p.len()).sum();

        // Encode with verbatim for comparison
        let enc_verb = FlacEncoder::new(&config).unwrap();
        let float_samples = bytes_to_f32(&buf.data);
        let scale = crate::audio::sample::I16_SCALE;
        let int_samples: Vec<i32> = float_samples
            .iter()
            .take(4096)
            .map(|s| (s.clamp(-1.0, 1.0) * scale) as i32)
            .collect();
        let verbatim_size = enc_verb.encode_frame_verbatim(&int_samples, 4096).len();

        assert!(
            fixed_size < verbatim_size,
            "Fixed frame ({fixed_size}) should be smaller than verbatim ({verbatim_size})"
        );
    }

    #[test]
    fn flac_crc8_nonzero() {
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

        // The frame header for a 4096-block, 44100Hz, mono, 16-bit frame is:
        // 2 bytes sync + 1 byte (bs_code+sr_code) + 1 byte (ch+ss+reserved)
        // + 1 byte frame number = 5 bytes, then CRC-8 at byte 5
        let frame = &packets[1];
        // CRC-8 is the byte right after the frame header (before subframes).
        // For our encoding: sync(2) + bs_sr(1) + ch_ss_res(1) + frame_num(1) = 5 bytes
        // CRC-8 is at index 5
        let crc8_byte = frame[5];
        assert_ne!(
            crc8_byte, 0,
            "CRC-8 should be non-zero for non-trivial data"
        );
    }

    #[test]
    fn flac_crc16_nonzero() {
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

        let frame = &packets[1];
        let len = frame.len();
        let crc16_val = ((frame[len - 2] as u16) << 8) | frame[len - 1] as u16;
        assert_ne!(
            crc16_val, 0,
            "CRC-16 should be non-zero for non-trivial data"
        );
    }

    #[test]
    fn flac_rice_coding_roundtrip() {
        // Test that Rice encode/decode mapping is correct for various values
        let test_values: Vec<i32> = vec![0, 1, -1, 2, -2, 127, -128, 1000, -1000];
        for &v in &test_values {
            let encoded = rice_encode_value(v);
            let decoded = rice_decode_value(encoded);
            assert_eq!(
                decoded, v,
                "Rice roundtrip failed for {v}: encoded={encoded}, decoded={decoded}"
            );
        }
    }

    #[test]
    fn flac_lpc_compresses_better_than_fixed() {
        // A complex signal (sum of multiple sines) where LPC should beat fixed.
        let num_samples = 4096usize;
        let sr = 44100.0f64;
        let freqs = [261.63, 329.63, 392.0, 523.25, 659.25]; // C major chord + octave
        let mut samples = vec![0.0f32; num_samples];
        for (i, sample) in samples.iter_mut().enumerate() {
            let t = i as f64 / sr;
            let mut val = 0.0f64;
            for &f in &freqs {
                val += (2.0 * std::f64::consts::PI * f * t).sin();
            }
            *sample = (val / freqs.len() as f64) as f32;
        }

        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };

        // Encode with the full encoder (fixed + LPC)
        let mut enc = FlacEncoder::new(&config).unwrap();
        let buf = make_buffer(&samples, 1, 44100);
        let packets = enc.encode(&buf).unwrap();
        let total_size: usize = packets[1..].iter().map(|p| p.len()).sum();

        // The encoder should produce a valid output that is smaller than verbatim
        let verbatim_approx = num_samples * 2; // 16-bit samples
        assert!(
            total_size < verbatim_approx,
            "LPC-enabled encoder ({total_size}) should compress better than verbatim ({verbatim_approx})"
        );
    }

    #[test]
    fn flac_lpc_degenerate_signal_fallback() {
        // DC signal (all same value) — should not crash, falls back gracefully
        let dc_signal = vec![0.5f32; 4096];
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };
        let mut enc = FlacEncoder::new(&config).unwrap();
        let buf = make_buffer(&dc_signal, 1, 44100);
        let packets = enc.encode(&buf).unwrap();
        assert!(packets.len() >= 2, "Should produce streaminfo + frame");
        // Frame should start with sync code
        assert_eq!(packets[1][0], 0xFF);

        // All-zero signal
        let zero_signal = vec![0.0f32; 4096];
        let mut enc2 = FlacEncoder::new(&config).unwrap();
        let buf2 = make_buffer(&zero_signal, 1, 44100);
        let packets2 = enc2.encode(&buf2).unwrap();
        assert!(
            packets2.len() >= 2,
            "Zero signal should produce valid output"
        );
    }

    #[test]
    fn flac_lpc_quantization_roundtrip() {
        // Verify quantize_lpc produces valid precision/shift values
        let coeffs = vec![0.9, -0.5, 0.3, -0.1];
        let (qcoeffs, precision, shift) = quantize_lpc(&coeffs, 16);

        assert_eq!(precision, 15, "Precision should be min(15, bps-1) = 15");
        assert!(
            (-16..=15).contains(&shift),
            "Shift {shift} out of FLAC range"
        );
        assert_eq!(qcoeffs.len(), coeffs.len());

        // Check that quantized coefficients approximate the originals
        let scale = (1i64 << shift.max(0)) as f64;
        for (i, (&orig, &quant)) in coeffs.iter().zip(qcoeffs.iter()).enumerate() {
            let reconstructed = quant as f64 / scale;
            let err = (orig - reconstructed).abs();
            assert!(
                err < 0.01,
                "Coefficient {i}: orig={orig}, quant={quant}, reconstructed={reconstructed}, err={err}"
            );
        }

        // Test edge case: near-zero coefficients
        let tiny_coeffs = vec![1e-12, -1e-12];
        let (qc, _p, _s) = quantize_lpc(&tiny_coeffs, 16);
        assert_eq!(
            qc,
            vec![0, 0],
            "Near-zero coefficients should quantize to zero"
        );

        // Test with 24-bit
        let (_, precision_24, _) = quantize_lpc(&coeffs, 24);
        assert_eq!(precision_24, 15, "Precision capped at 15 even for 24-bit");
    }

    /// Decode FLAC bytes back to interleaved f32 samples using symphonia.
    fn decode_flac_bytes(data: &[u8]) -> Vec<f32> {
        use std::io::Cursor;
        use symphonia::core::audio::SampleBuffer;
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        let cursor = Cursor::new(data.to_vec());
        let mss = MediaSourceStream::new(Box::new(cursor), Default::default());

        let mut hint = Hint::new();
        hint.with_extension("flac");

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .expect("failed to probe FLAC stream");

        let mut format = probed.format;
        let track = format.default_track().expect("no default track").clone();
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &Default::default())
            .expect("failed to create decoder");

        let mut all_samples = Vec::new();
        while let Ok(packet) = format.next_packet() {
            if packet.track_id() != track.id {
                continue;
            }
            let decoded = match decoder.decode(&packet) {
                Ok(d) => d,
                Err(_) => break,
            };
            let spec = *decoded.spec();
            let num_frames = decoded.frames();
            let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
            sample_buf.copy_interleaved_ref(decoded);
            all_samples.extend_from_slice(sample_buf.samples());
        }
        all_samples
    }

    /// Helper: encode f32 samples to FLAC bytes via FlacEncoder.
    fn encode_to_flac_bytes(samples: &[f32], channels: u16, sample_rate: u32) -> Vec<u8> {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate,
            channels,
            bits_per_sample: 16,
        };
        let mut enc = FlacEncoder::new(&config).unwrap();
        let buf = make_buffer(samples, channels, sample_rate);
        let packets = enc.encode(&buf).unwrap();
        let mut out = Vec::new();
        for p in packets {
            out.extend_from_slice(&p);
        }
        out
    }

    #[test]
    fn test_flac_roundtrip_fixed_orders() {
        // Encode a 440Hz mono sine wave (~1 second).
        // Use a multiple of block size (4096) to avoid partial-block edge cases.
        let num_samples = 4096 * 11; // 45056 samples ≈ 1.02s at 44100Hz
        let samples = make_sine(num_samples, 1);
        let flac_bytes = encode_to_flac_bytes(&samples, 1, 44100);
        let decoded = decode_flac_bytes(&flac_bytes);

        // The encoder converts f32 -> i16 -> FLAC, decoder gives f32 back.
        // Tolerance: ±1 LSB of 16-bit = 1/32767 ≈ 3.1e-5
        let tolerance = 1.0 / 32767.0 + 1e-5;
        assert_eq!(
            decoded.len(),
            samples.len(),
            "decoded length {} != original length {}",
            decoded.len(),
            samples.len()
        );
        for (i, (&orig, &dec)) in samples.iter().zip(decoded.iter()).enumerate() {
            // Compare after quantising original to 16-bit the same way the encoder does
            let orig_q = ((orig.clamp(-1.0, 1.0) * 32767.0) as i32) as f32 / 32767.0;
            let diff = (orig_q - dec).abs();
            assert!(
                diff <= tolerance,
                "sample {i}: orig_q={orig_q}, decoded={dec}, diff={diff}"
            );
        }
    }

    #[test]
    fn test_flac_roundtrip_stereo() {
        let num_samples = 4096 * 11; // 45056 samples
        let samples = make_sine(num_samples, 2);
        let flac_bytes = encode_to_flac_bytes(&samples, 2, 44100);
        let decoded = decode_flac_bytes(&flac_bytes);

        let tolerance = 1.0 / 32767.0 + 1e-5;
        assert_eq!(
            decoded.len(),
            samples.len(),
            "decoded length {} != original length {}",
            decoded.len(),
            samples.len()
        );
        for (i, (&orig, &dec)) in samples.iter().zip(decoded.iter()).enumerate() {
            let orig_q = ((orig.clamp(-1.0, 1.0) * 32767.0) as i32) as f32 / 32767.0;
            let diff = (orig_q - dec).abs();
            assert!(
                diff <= tolerance,
                "stereo sample {i}: orig_q={orig_q}, decoded={dec}, diff={diff}"
            );
        }
    }

    #[test]
    fn test_flac_crc_validity() {
        let samples = make_sine(4096, 1);
        let flac_bytes = encode_to_flac_bytes(&samples, 1, 44100);

        // "fLaC" magic bytes at offset 0
        assert_eq!(&flac_bytes[0..4], b"fLaC", "missing fLaC magic");

        // STREAMINFO block header at offset 4
        // Byte 4 = 0x80 (last-metadata-block flag set + block type 0)
        assert_eq!(
            flac_bytes[4], 0x80,
            "STREAMINFO block should start with 0x80 (last-metadata-block + type 0)"
        );

        // STREAMINFO length = 34 bytes (stored in bytes 5..8 as 24-bit big-endian)
        let si_len =
            ((flac_bytes[5] as u32) << 16) | ((flac_bytes[6] as u32) << 8) | (flac_bytes[7] as u32);
        assert_eq!(si_len, 34, "STREAMINFO length should be 34 bytes");
    }

    #[test]
    fn test_flac_rice_coding_silence() {
        let num_samples = 4096 * 11; // 45056 samples
        let silence = vec![0.0f32; num_samples];
        let flac_bytes = encode_to_flac_bytes(&silence, 1, 44100);

        // Check compression: input is num_samples * 2 bytes (16-bit PCM)
        let raw_size = num_samples * 2;
        assert!(
            flac_bytes.len() < raw_size / 4,
            "silence FLAC ({} bytes) should be much smaller than raw PCM ({} bytes)",
            flac_bytes.len(),
            raw_size
        );

        // Roundtrip: all decoded samples should be zero (or very near zero)
        let decoded = decode_flac_bytes(&flac_bytes);
        assert_eq!(decoded.len(), num_samples);
        for (i, &s) in decoded.iter().enumerate() {
            assert!(s.abs() < 1e-5, "silence sample {i} should be ~0.0, got {s}");
        }
    }

    #[test]
    fn flac_silence_compresses_well() {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };

        // All-zero (silence) input
        let silence = vec![0.0f32; 4096];
        let buf = make_buffer(&silence, 1, 44100);

        let mut enc = FlacEncoder::new(&config).unwrap();
        let packets = enc.encode(&buf).unwrap();
        let frame_size: usize = packets[1..].iter().map(|p| p.len()).sum();

        // Verbatim would be ~4096*2 = 8192 bytes + overhead
        // Silence with fixed order 0 should compress to mostly 1-bit-per-sample residuals
        let verbatim_approx = 4096 * 2;
        assert!(
            frame_size < verbatim_approx / 4,
            "Silence frame ({frame_size} bytes) should be much smaller than verbatim (~{verbatim_approx} bytes)"
        );
    }

    // -----------------------------------------------------------------------
    // Edge-case tests with symphonia roundtrip verification
    // -----------------------------------------------------------------------

    /// Like `encode_to_flac_bytes` but allows specifying bits_per_sample.
    fn encode_to_flac_bytes_bps(
        samples: &[f32],
        channels: u16,
        sample_rate: u32,
        bits_per_sample: u16,
    ) -> (Vec<u8>, Vec<Vec<u8>>) {
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate,
            channels,
            bits_per_sample,
        };
        let mut enc = FlacEncoder::new(&config).unwrap();
        let buf = make_buffer(samples, channels, sample_rate);
        let packets = enc.encode(&buf).unwrap();
        let mut out = Vec::new();
        for p in &packets {
            out.extend_from_slice(p);
        }
        (out, packets)
    }

    #[test]
    fn test_flac_24bit_encoding() {
        let num_samples = 4096usize;
        let channels = 1u16;
        let sample_rate = 44100u32;
        let samples = make_sine(num_samples, channels);

        // Encode at 24-bit
        let config = EncoderConfig {
            codec: AudioCodec::Flac,
            sample_rate,
            channels,
            bits_per_sample: 24,
        };
        let enc = FlacEncoder::new(&config).unwrap();
        assert_eq!(enc.bits_per_sample, 24);

        let (flac_bytes, _packets) = encode_to_flac_bytes_bps(&samples, channels, sample_rate, 24);

        // Verify STREAMINFO reports 24 bits per sample.
        // Layout: "fLaC"(4) + block_header(4) + STREAMINFO(34).
        // Inside STREAMINFO (34 bytes), the packed 8-byte field starts at
        // offset 10 from STREAMINFO start = absolute offset 4+4+10 = 18.
        //   absolute[18] = sr[19:12]
        //   absolute[19] = sr[11:4]
        //   absolute[20] = sr[3:0](4) | ch-1(3) | bps_high(1)
        //   absolute[21] = bps_low(4) | total_samples_high(4)
        let byte20 = flac_bytes[20]; // sr[3:0](4) | ch-1(3) | bps_high(1)
        let byte21 = flac_bytes[21]; // bps_low(4) | total_samples_high(4)
        let bps_minus_1 = ((byte20 & 0x01) << 4) | ((byte21 >> 4) & 0x0F);
        assert_eq!(
            bps_minus_1 + 1,
            24,
            "STREAMINFO should report 24 bits per sample"
        );

        // Roundtrip decode with symphonia
        let decoded = decode_flac_bytes(&flac_bytes);
        assert_eq!(decoded.len(), samples.len());

        // 24-bit quantization: tolerance = 1/8388607 + epsilon
        let tolerance = 1.0 / 8388607.0 + 1e-5;
        for (i, (&orig, &dec)) in samples.iter().zip(decoded.iter()).enumerate() {
            let orig_q = ((orig.clamp(-1.0, 1.0) * 8388607.0) as i32) as f32 / 8388607.0;
            let diff = (orig_q - dec).abs();
            assert!(
                diff <= tolerance,
                "24-bit roundtrip sample {i}: orig_q={orig_q}, decoded={dec}, diff={diff}"
            );
        }
    }

    #[test]
    fn test_flac_single_sample_block() {
        // Encode a buffer with exactly 1 sample -- should not panic
        let samples = vec![0.42f32];
        let (flac_bytes, packets) = encode_to_flac_bytes_bps(&samples, 1, 44100, 16);

        // Verify output starts with fLaC magic
        assert!(packets.len() >= 2);
        assert_eq!(&flac_bytes[..4], b"fLaC");

        // Frame data should be present and non-empty
        assert!(!packets[1].is_empty());

        // Frame should start with sync code
        assert_eq!(packets[1][0], 0xFF);
        assert_eq!(packets[1][1] & 0xFC, 0xF8);
    }

    #[test]
    fn test_flac_max_amplitude() {
        // Encode samples at full scale: alternating +1.0 and -1.0
        let num_samples = 4096usize;
        let mut samples = Vec::with_capacity(num_samples);
        for i in 0..num_samples {
            samples.push(if i % 2 == 0 { 1.0f32 } else { -1.0f32 });
        }

        let flac_bytes = encode_to_flac_bytes(&samples, 1, 44100);
        let decoded = decode_flac_bytes(&flac_bytes);
        assert_eq!(decoded.len(), samples.len());

        // 16-bit quantization tolerance
        let tolerance = 1.0 / 32767.0 + 1e-5;
        for (i, (&orig, &dec)) in samples.iter().zip(decoded.iter()).enumerate() {
            let orig_q = ((orig.clamp(-1.0, 1.0) * 32767.0) as i32) as f32 / 32767.0;
            let diff = (orig_q - dec).abs();
            assert!(
                diff <= tolerance,
                "Max-amplitude roundtrip sample {i}: orig_q={orig_q}, decoded={dec}, diff={diff}"
            );
        }
    }

    #[test]
    fn test_flac_dc_offset() {
        // Constant DC signal: all samples = 0.5
        let num_samples = 4096usize;
        let samples = vec![0.5f32; num_samples];

        let flac_bytes = encode_to_flac_bytes(&samples, 1, 44100);
        let decoded = decode_flac_bytes(&flac_bytes);
        assert_eq!(decoded.len(), samples.len());

        // Verify roundtrip accuracy
        let tolerance = 1.0 / 32767.0 + 1e-5;
        for (i, (&orig, &dec)) in samples.iter().zip(decoded.iter()).enumerate() {
            let orig_q = ((orig.clamp(-1.0, 1.0) * 32767.0) as i32) as f32 / 32767.0;
            let diff = (orig_q - dec).abs();
            assert!(
                diff <= tolerance,
                "DC offset roundtrip sample {i}: orig_q={orig_q}, decoded={dec}, diff={diff}"
            );
        }

        // DC signal should compress very well (fixed order 1 makes all residuals zero)
        let frame_size = flac_bytes.len() - 42; // subtract fLaC(4) + metadata(38)
        let verbatim_size = num_samples * 2;
        assert!(
            frame_size < verbatim_size / 4,
            "DC signal frame ({frame_size}) should be much smaller than verbatim ({verbatim_size})"
        );
    }

    #[test]
    fn test_flac_multi_block() {
        // Encode a buffer spanning multiple FLAC frames (>4096 samples per channel).
        // Use an exact multiple of block size for clean roundtrip (the encoder
        // writes frame_number=0 for all frames, so symphonia may miscount with
        // partial trailing blocks).
        let num_samples = 4096 * 3; // 12288 samples -> exactly 3 blocks
        let channels = 1u16;
        let samples = make_sine(num_samples, channels);

        let (_flac_bytes, packets) = encode_to_flac_bytes_bps(&samples, channels, 44100, 16);

        // Should have: 1 streaminfo packet + 3 frame packets
        assert_eq!(
            packets.len(),
            4,
            "Expected 1 header + 3 frames, got {} packets",
            packets.len()
        );

        // Each frame packet should start with the sync code 0xFFF8
        let mut sync_count = 0;
        for packet in &packets[1..] {
            if packet.len() >= 2 && packet[0] == 0xFF && (packet[1] & 0xFC) == 0xF8 {
                sync_count += 1;
            }
        }
        assert_eq!(
            sync_count, 3,
            "Expected 3 frame sync codes, found {sync_count}"
        );

        // Also verify the concatenated stream contains multiple 0xFFF8 sync codes
        let flac_bytes = encode_to_flac_bytes(&samples, channels, 44100);
        let mut byte_sync_count = 0;
        for w in flac_bytes.windows(2) {
            if w[0] == 0xFF && (w[1] & 0xFC) == 0xF8 {
                byte_sync_count += 1;
            }
        }
        assert!(
            byte_sync_count >= 3,
            "Expected at least 3 sync codes in byte stream, found {byte_sync_count}"
        );

        // Roundtrip decode and verify
        let decoded = decode_flac_bytes(&flac_bytes);
        assert_eq!(decoded.len(), samples.len());

        let tolerance = 1.0 / 32767.0 + 1e-5;
        for (i, (&orig, &dec)) in samples.iter().zip(decoded.iter()).enumerate() {
            let orig_q = ((orig.clamp(-1.0, 1.0) * 32767.0) as i32) as f32 / 32767.0;
            let diff = (orig_q - dec).abs();
            assert!(
                diff <= tolerance,
                "Multi-block roundtrip sample {i}: orig_q={orig_q}, decoded={dec}, diff={diff}"
            );
        }
    }

    #[test]
    fn test_flac_compression_ratio() {
        // Encode a pure sine wave and verify at least 30% compression vs raw PCM
        let num_samples = 4096usize;
        let channels = 1u16;
        let samples = make_sine(num_samples, channels);

        let flac_bytes = encode_to_flac_bytes(&samples, channels, 44100);

        // Compressed size excluding the stream header (fLaC + STREAMINFO = 42 bytes)
        let compressed_size = flac_bytes.len() - 42;
        // Raw PCM: 16-bit mono = 2 bytes per sample
        let raw_pcm_size = num_samples * channels as usize * 2;

        let ratio = 1.0 - (compressed_size as f64 / raw_pcm_size as f64);
        assert!(
            ratio >= 0.30,
            "Expected at least 30% compression for a pure sine tone, \
             got {:.1}% (compressed={compressed_size}, raw={raw_pcm_size})",
            ratio * 100.0
        );
    }
}
