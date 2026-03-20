//! Audio output backends
//!
//! `AudioOutput` trait abstracts over output sinks. The PipeWire backend
//! is available behind the `pipewire` feature flag.

use crate::core::{AudioBuffer, Result, TarangError};

/// Configuration for an audio output stream
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_size: usize,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            sample_rate: 44100,
            channels: 2,
            buffer_size: 1024,
        }
    }
}

/// Trait for audio output backends
pub trait AudioOutput {
    /// Open the output stream with the given configuration.
    fn open(&mut self, config: &OutputConfig) -> Result<()>;

    /// Write a decoded audio buffer to the output.
    /// Blocks until the data is consumed or buffered.
    fn write(&mut self, buf: &AudioBuffer) -> Result<()>;

    /// Flush any buffered data and wait for playback to finish.
    fn flush(&mut self) -> Result<()>;

    /// Close the output stream.
    fn close(&mut self) -> Result<()>;

    /// Current playback latency estimate.
    fn latency(&self) -> std::time::Duration;
}

// ---- PipeWire backend ----

#[cfg(feature = "pipewire")]
mod pw;

#[cfg(feature = "pipewire")]
pub use pw::PipeWireOutput;

// ---- Null output (always available, useful for testing/benchmarks) ----

/// A no-op audio output that discards all samples.
/// Useful for testing decode pipelines without requiring audio hardware.
pub struct NullOutput {
    config: Option<OutputConfig>,
    samples_written: u64,
}

impl Default for NullOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl NullOutput {
    pub fn new() -> Self {
        Self {
            config: None,
            samples_written: 0,
        }
    }

    pub fn samples_written(&self) -> u64 {
        self.samples_written
    }
}

impl AudioOutput for NullOutput {
    fn open(&mut self, config: &OutputConfig) -> Result<()> {
        self.config = Some(*config);
        self.samples_written = 0;
        Ok(())
    }

    fn write(&mut self, buf: &AudioBuffer) -> Result<()> {
        if self.config.is_none() {
            return Err(TarangError::Pipeline("output not opened".into()));
        }
        self.samples_written += buf.num_frames as u64;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        self.config = None;
        Ok(())
    }

    fn latency(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SampleFormat;
    use bytes::Bytes;
    use std::time::Duration;

    fn make_buffer(num_frames: usize, channels: u16, sample_rate: u32) -> AudioBuffer {
        let data = vec![0.5f32; num_frames * channels as usize];
        AudioBuffer {
            data: Bytes::copy_from_slice(crate::audio::sample::f32_to_bytes(&data)),
            sample_format: SampleFormat::F32,
            channels,
            sample_rate,
            num_frames,
            timestamp: Duration::ZERO,
        }
    }

    #[test]
    fn null_output_basic() {
        let mut out = NullOutput::new();
        let config = OutputConfig {
            sample_rate: 44100,
            channels: 2,
            buffer_size: 1024,
        };
        out.open(&config).unwrap();

        let buf = make_buffer(1024, 2, 44100);
        out.write(&buf).unwrap();
        assert_eq!(out.samples_written(), 1024);

        out.write(&buf).unwrap();
        assert_eq!(out.samples_written(), 2048);

        out.flush().unwrap();
        out.close().unwrap();
    }

    #[test]
    fn null_output_write_before_open() {
        let mut out = NullOutput::new();
        let buf = make_buffer(100, 2, 44100);
        assert!(out.write(&buf).is_err());
    }

    #[test]
    fn null_output_latency() {
        let out = NullOutput::new();
        assert_eq!(out.latency(), Duration::ZERO);
    }

    #[test]
    fn output_config_default() {
        let config = OutputConfig::default();
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.channels, 2);
        assert_eq!(config.buffer_size, 1024);
    }
}
