//! Composable audio effects pipeline.
//!
//! Provides a trait-based framework for chaining audio transforms.
//! Effects process `AudioBuffer`s and can be composed into chains.
//!
//! ```rust,no_run
//! # use tarang::audio::effects::*;
//! let mut chain = EffectChain::new();
//! chain.add(Box::new(Gain::new(-3.0)));  // -3 dB
//! chain.add(Box::new(HighPassFilter::new(80.0)));
//! // let output = chain.process(&input_buffer).unwrap();
//! ```

use crate::core::{AudioBuffer, Result};

/// Trait for audio effects that process buffers.
pub trait AudioEffect: Send {
    /// Process an audio buffer, returning the transformed output.
    fn process(&mut self, buf: &AudioBuffer) -> Result<AudioBuffer>;

    /// Human-readable name for this effect.
    fn name(&self) -> &str;
}

/// A chain of effects applied sequentially.
pub struct EffectChain {
    effects: Vec<Box<dyn AudioEffect>>,
}

impl EffectChain {
    /// Create an empty effect chain.
    pub fn new() -> Self {
        Self {
            effects: Vec::new(),
        }
    }

    /// Add an effect to the end of the chain.
    pub fn add(&mut self, effect: Box<dyn AudioEffect>) {
        self.effects.push(effect);
    }

    /// Process a buffer through all effects in order.
    pub fn process(&mut self, buf: &AudioBuffer) -> Result<AudioBuffer> {
        let mut current = buf.clone();
        for effect in &mut self.effects {
            current = effect.process(&current)?;
        }
        Ok(current)
    }

    /// Number of effects in the chain.
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

impl Default for EffectChain {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Built-in effects
// ---------------------------------------------------------------------------

/// Apply a gain (volume change) in decibels.
pub struct Gain {
    /// Linear gain multiplier (computed from dB).
    multiplier: f32,
    _db: f32,
}

impl Gain {
    /// Create a gain effect. Positive dB = louder, negative = quieter.
    pub fn new(db: f32) -> Self {
        Self {
            multiplier: 10.0f32.powf(db / 20.0),
            _db: db,
        }
    }
}

impl AudioEffect for Gain {
    fn process(&mut self, buf: &AudioBuffer) -> Result<AudioBuffer> {
        let samples = crate::audio::sample::bytes_to_f32(&buf.data);
        let out: Vec<f32> = samples.iter().map(|&s| s * self.multiplier).collect();
        Ok(AudioBuffer {
            data: crate::audio::sample::f32_vec_into_bytes(out),
            sample_format: buf.sample_format,
            channels: buf.channels,
            sample_rate: buf.sample_rate,
            num_frames: buf.num_frames,
            timestamp: buf.timestamp,
        })
    }

    fn name(&self) -> &str {
        "gain"
    }
}

/// Simple first-order high-pass filter (removes frequencies below cutoff).
pub struct HighPassFilter {
    cutoff_hz: f32,
    /// Per-channel state (previous input and output).
    prev_in: Vec<f32>,
    prev_out: Vec<f32>,
    alpha: f32,
    initialized: bool,
}

impl HighPassFilter {
    /// Create a high-pass filter with the given cutoff frequency.
    pub fn new(cutoff_hz: f32) -> Self {
        Self {
            cutoff_hz,
            prev_in: Vec::new(),
            prev_out: Vec::new(),
            alpha: 0.0,
            initialized: false,
        }
    }

    fn init(&mut self, sample_rate: u32, channels: u16) {
        let rc = 1.0 / (2.0 * std::f32::consts::PI * self.cutoff_hz);
        let dt = 1.0 / sample_rate as f32;
        self.alpha = rc / (rc + dt);
        self.prev_in = vec![0.0; channels as usize];
        self.prev_out = vec![0.0; channels as usize];
        self.initialized = true;
    }
}

impl AudioEffect for HighPassFilter {
    fn process(&mut self, buf: &AudioBuffer) -> Result<AudioBuffer> {
        if !self.initialized {
            self.init(buf.sample_rate, buf.channels);
        }

        let samples = crate::audio::sample::bytes_to_f32(&buf.data);
        let ch = buf.channels as usize;
        let mut out = Vec::with_capacity(samples.len());

        for frame_samples in samples.chunks(ch) {
            for (c, &s) in frame_samples.iter().enumerate() {
                let y = self.alpha * (self.prev_out[c] + s - self.prev_in[c]);
                self.prev_in[c] = s;
                self.prev_out[c] = y;
                out.push(y);
            }
        }

        Ok(AudioBuffer {
            data: crate::audio::sample::f32_vec_into_bytes(out),
            sample_format: buf.sample_format,
            channels: buf.channels,
            sample_rate: buf.sample_rate,
            num_frames: buf.num_frames,
            timestamp: buf.timestamp,
        })
    }

    fn name(&self) -> &str {
        "high_pass_filter"
    }
}

/// Simple dynamic range compressor.
pub struct Compressor {
    _threshold_db: f32,
    ratio: f32,
    /// Linear threshold.
    threshold_lin: f32,
}

impl Compressor {
    /// Create a compressor. `threshold_db` is the level above which
    /// compression applies. `ratio` is the compression ratio (e.g. 4.0 = 4:1).
    pub fn new(threshold_db: f32, ratio: f32) -> Self {
        Self {
            _threshold_db: threshold_db,
            ratio: ratio.max(1.0),
            threshold_lin: 10.0f32.powf(threshold_db / 20.0),
        }
    }
}

impl AudioEffect for Compressor {
    fn process(&mut self, buf: &AudioBuffer) -> Result<AudioBuffer> {
        let samples = crate::audio::sample::bytes_to_f32(&buf.data);
        let out: Vec<f32> = samples
            .iter()
            .map(|&s| {
                let abs = s.abs();
                if abs > self.threshold_lin {
                    let excess = abs - self.threshold_lin;
                    let compressed = self.threshold_lin + excess / self.ratio;
                    compressed.copysign(s)
                } else {
                    s
                }
            })
            .collect();

        Ok(AudioBuffer {
            data: crate::audio::sample::f32_vec_into_bytes(out),
            sample_format: buf.sample_format,
            channels: buf.channels,
            sample_rate: buf.sample_rate,
            num_frames: buf.num_frames,
            timestamp: buf.timestamp,
        })
    }

    fn name(&self) -> &str {
        "compressor"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sample::make_test_buffer;

    #[test]
    fn gain_positive() {
        let buf = make_test_buffer(&[0.5, -0.5], 1, 44100);
        let mut gain = Gain::new(6.0); // ~2x
        let out = gain.process(&buf).unwrap();
        let samples = crate::audio::sample::bytes_to_f32(&out.data);
        assert!(samples[0] > 0.9); // 0.5 * ~2 ≈ 1.0
    }

    #[test]
    fn gain_negative() {
        let buf = make_test_buffer(&[1.0, -1.0], 1, 44100);
        let mut gain = Gain::new(-6.0); // ~0.5x
        let out = gain.process(&buf).unwrap();
        let samples = crate::audio::sample::bytes_to_f32(&out.data);
        assert!(samples[0] < 0.6);
        assert!(samples[0] > 0.4);
    }

    #[test]
    fn gain_zero_db_passthrough() {
        let buf = make_test_buffer(&[0.42, -0.42], 1, 44100);
        let mut gain = Gain::new(0.0);
        let out = gain.process(&buf).unwrap();
        let samples = crate::audio::sample::bytes_to_f32(&out.data);
        assert!((samples[0] - 0.42).abs() < 1e-6);
    }

    #[test]
    fn effect_chain_empty() {
        let buf = make_test_buffer(&[0.5], 1, 44100);
        let mut chain = EffectChain::new();
        assert!(chain.is_empty());
        let out = chain.process(&buf).unwrap();
        let samples = crate::audio::sample::bytes_to_f32(&out.data);
        assert!((samples[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn effect_chain_multiple() {
        let buf = make_test_buffer(&[0.5, -0.5, 0.3, -0.3], 1, 44100);
        let mut chain = EffectChain::new();
        chain.add(Box::new(Gain::new(6.0)));
        chain.add(Box::new(Gain::new(-6.0))); // should cancel out
        assert_eq!(chain.len(), 2);
        let out = chain.process(&buf).unwrap();
        let samples = crate::audio::sample::bytes_to_f32(&out.data);
        // +6dB then -6dB ≈ identity (small floating point error)
        assert!((samples[0] - 0.5).abs() < 0.01);
    }

    #[test]
    fn high_pass_filter_removes_dc() {
        // DC signal (constant 0.5) should be attenuated by HPF
        let dc = vec![0.5f32; 4410]; // 0.1 sec at 44100Hz
        let buf = make_test_buffer(&dc, 1, 44100);
        let mut hpf = HighPassFilter::new(100.0);
        let out = hpf.process(&buf).unwrap();
        let samples = crate::audio::sample::bytes_to_f32(&out.data);
        // Last sample should be near 0 (DC removed)
        assert!(
            samples.last().unwrap().abs() < 0.1,
            "HPF should attenuate DC"
        );
    }

    #[test]
    fn compressor_leaves_quiet_signals() {
        let buf = make_test_buffer(&[0.1, -0.1, 0.05], 1, 44100);
        let mut comp = Compressor::new(-6.0, 4.0); // threshold ≈ 0.5
        let out = comp.process(&buf).unwrap();
        let samples = crate::audio::sample::bytes_to_f32(&out.data);
        // Below threshold — unchanged
        assert!((samples[0] - 0.1).abs() < 1e-6);
    }

    #[test]
    fn compressor_reduces_loud_signals() {
        let buf = make_test_buffer(&[0.9, -0.9], 1, 44100);
        let mut comp = Compressor::new(-6.0, 4.0); // threshold ≈ 0.5
        let out = comp.process(&buf).unwrap();
        let samples = crate::audio::sample::bytes_to_f32(&out.data);
        assert!(samples[0].abs() < 0.9, "compressor should reduce level");
        assert!(samples[0].abs() > 0.5, "should still be above threshold");
    }
}
