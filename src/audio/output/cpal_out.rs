//! Cross-platform audio output via cpal
//!
//! Supports CoreAudio (macOS), WASAPI (Windows), and ALSA (Linux) through
//! the `cpal` crate. Enable with the `cpal-output` feature flag.

use super::{AudioOutput, OutputConfig};
use crate::core::{AudioBuffer, Result, TarangError};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

/// Cross-platform audio output using cpal (CoreAudio, WASAPI, ALSA).
pub struct CpalOutput {
    config: Option<OutputConfig>,
    stream: Option<cpal::Stream>,
    ring: Arc<Mutex<RingBuffer>>,
}

/// Simple ring buffer for feeding samples to the cpal callback.
struct RingBuffer {
    data: Vec<f32>,
    read_pos: usize,
    write_pos: usize,
    len: usize,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            data: vec![0.0; capacity],
            read_pos: 0,
            write_pos: 0,
            len: 0,
        }
    }

    fn push(&mut self, samples: &[f32]) -> usize {
        if self.data.is_empty() {
            return 0;
        }
        let available = self.data.len() - self.len;
        let to_write = samples.len().min(available);
        for &s in &samples[..to_write] {
            self.data[self.write_pos] = s;
            self.write_pos = (self.write_pos + 1) % self.data.len();
        }
        self.len += to_write;
        to_write
    }

    fn pop(&mut self, out: &mut [f32]) -> usize {
        if self.data.is_empty() {
            for sample in out.iter_mut() {
                *sample = 0.0;
            }
            return 0;
        }
        let to_read = out.len().min(self.len);
        for sample in &mut out[..to_read] {
            *sample = self.data[self.read_pos];
            self.read_pos = (self.read_pos + 1) % self.data.len();
        }
        // Zero-fill remainder (silence)
        for sample in &mut out[to_read..] {
            *sample = 0.0;
        }
        self.len -= to_read;
        to_read
    }

    fn len(&self) -> usize {
        self.len
    }
}

impl Default for CpalOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl CpalOutput {
    pub fn new() -> Self {
        Self {
            config: None,
            stream: None,
            ring: Arc::new(Mutex::new(RingBuffer::new(0))),
        }
    }
}

impl AudioOutput for CpalOutput {
    fn open(&mut self, config: &OutputConfig) -> Result<()> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| TarangError::Pipeline("no audio output device available".into()))?;

        let stream_config = cpal::StreamConfig {
            channels: config.channels,
            sample_rate: cpal::SampleRate(config.sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        // Ring buffer: ~500ms of audio
        let ring_capacity = config.sample_rate as usize * config.channels as usize / 2;
        let ring = Arc::new(Mutex::new(RingBuffer::new(ring_capacity)));
        self.ring = Arc::clone(&ring);

        let stream = device
            .build_output_stream(
                &stream_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if let Ok(mut rb) = ring.lock() {
                        rb.pop(data);
                    }
                },
                |err| {
                    tracing::error!("cpal output stream error: {err}");
                },
                None,
            )
            .map_err(|e| TarangError::Pipeline(format!("cpal stream error: {e}").into()))?;

        stream
            .play()
            .map_err(|e| TarangError::Pipeline(format!("cpal play error: {e}").into()))?;

        self.stream = Some(stream);
        self.config = Some(*config);
        Ok(())
    }

    fn write(&mut self, buf: &AudioBuffer) -> Result<()> {
        if self.config.is_none() {
            return Err(TarangError::Pipeline("output not opened".into()));
        }
        let samples = crate::audio::sample::bytes_to_f32(&buf.data);
        let mut offset = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while offset < samples.len() {
            let written = {
                let mut rb = self
                    .ring
                    .lock()
                    .map_err(|_| TarangError::Pipeline("ring buffer lock poisoned".into()))?;
                rb.push(&samples[offset..])
            };
            offset += written;
            if offset < samples.len() {
                if std::time::Instant::now() >= deadline {
                    return Err(TarangError::Pipeline(
                        "audio output write timed out (ring buffer not draining)".into(),
                    ));
                }
                // Ring full — yield to let the output callback drain
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        // Wait for ring buffer to drain (timeout after 5s)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let len = {
                let rb = self
                    .ring
                    .lock()
                    .map_err(|_| TarangError::Pipeline("ring buffer lock poisoned".into()))?;
                rb.len()
            };
            if len == 0 {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(TarangError::Pipeline(
                    "audio output flush timed out (ring buffer not draining)".into(),
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        self.flush()?;
        self.stream = None;
        self.config = None;
        Ok(())
    }

    fn latency(&self) -> std::time::Duration {
        let len = self.ring.lock().map(|rb| rb.len()).unwrap_or(0);
        let config = self.config.unwrap_or_default();
        let frames = len / config.channels.max(1) as usize;
        let sample_rate = config.sample_rate.max(1);
        std::time::Duration::from_secs_f64(frames as f64 / sample_rate as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_push_pop() {
        let mut rb = RingBuffer::new(8);
        assert_eq!(rb.len(), 0);

        let written = rb.push(&[1.0, 2.0, 3.0]);
        assert_eq!(written, 3);
        assert_eq!(rb.len(), 3);

        let mut out = [0.0f32; 4];
        let read = rb.pop(&mut out);
        assert_eq!(read, 3);
        assert_eq!(out[0], 1.0);
        assert_eq!(out[1], 2.0);
        assert_eq!(out[2], 3.0);
        assert_eq!(out[3], 0.0); // zero-filled
        assert_eq!(rb.len(), 0);
    }

    #[test]
    fn ring_buffer_wraps_around() {
        let mut rb = RingBuffer::new(4);
        rb.push(&[1.0, 2.0, 3.0]);
        let mut out = [0.0f32; 2];
        rb.pop(&mut out); // consume 2, leaving [3.0]
        assert_eq!(rb.len(), 1);

        // Now write 3 more — should wrap around the 4-element buffer
        let written = rb.push(&[4.0, 5.0, 6.0]);
        assert_eq!(written, 3);
        assert_eq!(rb.len(), 4); // full

        let mut out2 = [0.0f32; 4];
        rb.pop(&mut out2);
        assert_eq!(out2, [3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn ring_buffer_full_rejects_overflow() {
        let mut rb = RingBuffer::new(3);
        let written = rb.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(written, 3); // only capacity
        assert_eq!(rb.len(), 3);

        // Push when full returns 0
        let written = rb.push(&[99.0]);
        assert_eq!(written, 0);
    }

    #[test]
    fn ring_buffer_empty_pop() {
        let mut rb = RingBuffer::new(4);
        let mut out = [99.0f32; 3];
        let read = rb.pop(&mut out);
        assert_eq!(read, 0);
        // Should be zero-filled
        assert_eq!(out, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn cpal_output_write_before_open() {
        use bytes::Bytes;
        use std::time::Duration;
        let mut out = CpalOutput::new();
        let buf = crate::core::AudioBuffer {
            data: Bytes::from(vec![0u8; 16]),
            sample_format: crate::core::SampleFormat::F32,
            channels: 1,
            sample_rate: 44100,
            num_frames: 4,
            timestamp: Duration::ZERO,
        };
        assert!(out.write(&buf).is_err());
    }
}
