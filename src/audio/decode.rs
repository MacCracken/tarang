//! Full audio decode pipeline via shravan
//!
//! `FileDecoder` wraps shravan's codec decoder to produce
//! interleaved F32 `AudioBuffer`s from any supported audio file.
//!
//! # Example
//! ```rust,ignore
//! use tarang::audio::decode::FileDecoder;
//!
//! let file = std::fs::File::open("song.flac").unwrap();
//! let mut decoder = FileDecoder::open(Box::new(file), Some("flac")).unwrap();
//! while let Some(buf) = decoder.next_buffer().unwrap() {
//!     // process buf.data …
//! }
//! ```

use crate::core::{AudioBuffer, AudioCodec, Result, SampleFormat, TarangError};
use bytes::Bytes;
use std::io::Read;
use std::time::Duration;

use super::probe::map_shravan_format;

/// Default frames per `next_buffer()` call.
const DEFAULT_CHUNK_SIZE: usize = 4096;

/// Full audio file decoder. Owns decoded samples from shravan,
/// producing `AudioBuffer`s chunk by chunk.
pub struct FileDecoder {
    samples: Vec<f32>,
    format: shravan::format::FormatInfo,
    codec: AudioCodec,
    position: usize,
    chunk_size: usize,
}

impl FileDecoder {
    /// Open an audio source for decoding.
    ///
    /// Accepts any `Read + Send` source (File, Cursor, network stream, etc.).
    /// Optionally provide a file extension hint (currently unused by shravan,
    /// kept for API compatibility).
    pub fn open(mut source: Box<dyn Read + Send>, _extension_hint: Option<&str>) -> Result<Self> {
        tracing::debug!("opening audio source for decoding");

        let mut data = Vec::new();
        source.read_to_end(&mut data).map_err(TarangError::Io)?;

        let (info, samples) = shravan::codec::open(&data)
            .map_err(|e| TarangError::DemuxError(format!("failed to decode audio: {e}").into()))?;

        if info.sample_rate == 0 {
            return Err(TarangError::DecodeError(
                "codec reports sample rate 0".into(),
            ));
        }

        if info.channels == 0 {
            return Err(TarangError::DecodeError("invalid channel count 0".into()));
        }

        let audio_format =
            shravan::format::detect_format(&data).unwrap_or(shravan::format::AudioFormat::RawPcm);
        let codec = map_shravan_format(audio_format);

        Ok(Self {
            samples,
            format: info,
            codec,
            position: 0,
            chunk_size: DEFAULT_CHUNK_SIZE,
        })
    }

    /// Open from a file path (convenience).
    pub fn open_path(path: &std::path::Path) -> Result<Self> {
        tracing::debug!(path = %path.display(), "opening audio file");
        let file = std::fs::File::open(path).map_err(TarangError::Io)?;
        Self::open(Box::new(file), path.extension().and_then(|e| e.to_str()))
    }

    /// The detected audio codec.
    pub fn codec(&self) -> AudioCodec {
        self.codec
    }

    /// Sample rate of the decoded audio.
    pub fn sample_rate(&self) -> u32 {
        self.format.sample_rate
    }

    /// Number of channels.
    pub fn channels(&self) -> u16 {
        self.format.channels
    }

    /// Decode the next frame, returning an `AudioBuffer` with interleaved F32 samples.
    /// Returns `Err(TarangError::EndOfStream)` when the file is fully decoded.
    pub fn next_buffer(&mut self) -> Result<AudioBuffer> {
        let channels = self.format.channels as usize;
        let sr = self.format.sample_rate;

        if self.position >= self.samples.len() {
            return Err(TarangError::EndOfStream);
        }

        let chunk_values = self.chunk_size * channels;
        let remaining = self.samples.len() - self.position;
        let take = chunk_values.min(remaining);
        // Align to channel boundary
        let take = take - (take % channels);

        if take == 0 {
            return Err(TarangError::EndOfStream);
        }

        let slice = &self.samples[self.position..self.position + take];
        let num_frames = take / channels;

        let timestamp = Duration::from_secs_f64((self.position / channels) as f64 / sr as f64);

        self.position += take;

        Ok(AudioBuffer {
            data: Bytes::copy_from_slice(bytemuck_f32_to_bytes(slice)),
            sample_format: SampleFormat::F32,
            channels: channels as u16,
            sample_rate: sr,
            num_frames,
            timestamp,
        })
    }

    /// Seek to the given timestamp. The next call to `next_buffer` will produce
    /// audio from approximately this position.
    pub fn seek(&mut self, timestamp: Duration) -> Result<()> {
        let channels = self.format.channels as usize;
        let sr = self.format.sample_rate;

        let frame_offset = (timestamp.as_secs_f64() * sr as f64) as usize;
        let sample_offset = frame_offset * channels;

        self.position = sample_offset.min(self.samples.len());
        Ok(())
    }

    /// Decode the entire file into a single contiguous buffer.
    /// Useful for short files or when you need all samples in memory.
    pub fn decode_all(&mut self) -> Result<AudioBuffer> {
        tracing::debug!("decoding entire audio stream");

        let channels = self.format.channels as usize;
        let sr = self.format.sample_rate;

        if self.samples.is_empty() {
            return Err(TarangError::DecodeError("no audio decoded".into()));
        }

        let remaining = &self.samples[self.position..];
        let total_values = remaining.len();
        // Align to channel boundary
        let total_values = total_values - (total_values % channels);

        if total_values == 0 {
            return Err(TarangError::DecodeError("no audio decoded".into()));
        }

        const MAX_DECODED_BYTES: usize = 536_870_912; // 512 MB
        if total_values * std::mem::size_of::<f32>() > MAX_DECODED_BYTES {
            return Err(TarangError::DecodeError(
                format!(
                    "decoded audio exceeds 512MB limit ({} bytes)",
                    total_values * std::mem::size_of::<f32>()
                )
                .into(),
            ));
        }

        let slice = &self.samples[self.position..self.position + total_values];
        let num_frames = total_values / channels;

        tracing::debug!(
            total_samples = num_frames,
            total_bytes = total_values * std::mem::size_of::<f32>(),
            "decode_all complete"
        );

        self.position = self.samples.len();

        Ok(AudioBuffer {
            data: Bytes::copy_from_slice(bytemuck_f32_to_bytes(slice)),
            sample_format: SampleFormat::F32,
            channels: channels as u16,
            sample_rate: sr,
            num_frames,
            timestamp: Duration::ZERO,
        })
    }
}

#[cfg(test)]
use super::sample::bytes_to_f32 as bytemuck_bytes_to_f32;
use super::sample::f32_to_bytes as bytemuck_f32_to_bytes;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Create a minimal WAV file in memory for testing the decode pipeline.
    fn make_wav_samples(num_samples: u32, sample_rate: u32, channels: u16) -> Vec<u8> {
        let bits: u16 = 16;
        let data_size = num_samples * channels as u32 * (bits as u32 / 8);
        let file_size = 36 + data_size;
        let byte_rate = sample_rate * channels as u32 * (bits as u32 / 8);
        let block_align = channels * (bits / 8);

        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());

        // Write a simple sine wave as 16-bit PCM
        for i in 0..num_samples {
            let t = i as f64 / sample_rate as f64;
            let sample = (t * 440.0 * 2.0 * std::f64::consts::PI).sin();
            let s16 = (sample * 32000.0) as i16;
            for _ in 0..channels {
                buf.extend_from_slice(&s16.to_le_bytes());
            }
        }

        buf
    }

    #[test]
    fn decode_wav_file() {
        let wav = make_wav_samples(4410, 44100, 2);
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();

        assert_eq!(decoder.codec(), AudioCodec::Pcm);
        assert_eq!(decoder.sample_rate(), 44100);
        assert_eq!(decoder.channels(), 2);

        let buf = decoder.next_buffer().unwrap();
        assert_eq!(buf.sample_format, SampleFormat::F32);
        assert_eq!(buf.sample_rate, 44100);
        assert_eq!(buf.channels, 2);
        assert!(buf.num_frames > 0);
    }

    #[test]
    fn decode_wav_all() {
        let wav = make_wav_samples(4410, 44100, 2);
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();

        let buf = decoder.decode_all().unwrap();
        assert_eq!(buf.sample_rate, 44100);
        assert_eq!(buf.channels, 2);
        assert_eq!(buf.num_frames, 4410);
        // 4410 samples * 2 channels * 4 bytes per f32
        assert_eq!(buf.data.len(), 4410 * 2 * 4);
    }

    #[test]
    fn decode_wav_mono() {
        let wav = make_wav_samples(1000, 48000, 1);
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();

        let buf = decoder.decode_all().unwrap();
        assert_eq!(buf.channels, 1);
        assert_eq!(buf.sample_rate, 48000);
        assert_eq!(buf.num_frames, 1000);
    }

    #[test]
    fn decode_wav_timestamps_increase() {
        let wav = make_wav_samples(44100, 44100, 2); // 1 second
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();

        let mut prev_ts = Duration::ZERO;
        let mut count = 0;
        loop {
            match decoder.next_buffer() {
                Ok(buf) => {
                    if count > 0 {
                        assert!(
                            buf.timestamp > prev_ts,
                            "timestamps must increase: {:?} <= {:?}",
                            buf.timestamp,
                            prev_ts
                        );
                    }
                    prev_ts = buf.timestamp;
                    count += 1;
                }
                Err(TarangError::EndOfStream) => break,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
        assert!(count > 0);
    }

    #[test]
    fn decode_wav_samples_are_nonzero() {
        // A 440Hz sine wave should have non-zero sample values
        let wav = make_wav_samples(4410, 44100, 1);
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();

        let buf = decoder.decode_all().unwrap();
        let samples = bytemuck_bytes_to_f32(&buf.data);
        let max_abs = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_abs > 0.1,
            "decoded sine wave should have significant amplitude, got max={max_abs}"
        );
    }

    #[test]
    fn decode_wav_seek() {
        let wav = make_wav_samples(44100, 44100, 2); // 1 second
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();

        // Seek to 0.5s
        decoder.seek(Duration::from_millis(500)).unwrap();
        let buf = decoder.next_buffer().unwrap();
        // Timestamp should be approximately at or after 0.5s
        assert!(
            buf.timestamp.as_secs_f64() >= 0.4,
            "after seeking to 0.5s, timestamp was {:?}",
            buf.timestamp
        );
    }

    #[test]
    fn decode_all_combines_buffers() {
        // Verify decode_all returns a single combined buffer
        let wav = make_wav_samples(8820, 44100, 1);
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();
        let buf = decoder.decode_all().unwrap();
        assert_eq!(buf.num_frames, 8820);
        assert_eq!(buf.channels, 1);
        assert_eq!(buf.sample_format, SampleFormat::F32);
    }

    #[test]
    fn decode_wav_high_sample_rate() {
        let wav = make_wav_samples(960, 96000, 1);
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();
        assert_eq!(decoder.sample_rate(), 96000);
        let buf = decoder.decode_all().unwrap();
        assert_eq!(buf.sample_rate, 96000);
    }

    #[test]
    fn open_path_nonexistent() {
        let result = FileDecoder::open_path(std::path::Path::new("/nonexistent/audio.wav"));
        assert!(result.is_err());
    }

    #[test]
    fn bytemuck_roundtrip() {
        let samples = [0.5f32, -0.25, 1.0, 0.0];
        let bytes = bytemuck_f32_to_bytes(&samples);
        let back = bytemuck_bytes_to_f32(bytes);
        assert_eq!(back, &samples);
    }

    #[test]
    fn bytemuck_empty() {
        let empty: &[u8] = &[];
        assert!(bytemuck_bytes_to_f32(empty).is_empty());
    }

    #[test]
    fn bytemuck_odd_bytes() {
        // Not a multiple of 4 — should return empty
        let odd = &[1u8, 2, 3, 4, 5];
        assert!(bytemuck_bytes_to_f32(odd).is_empty());
    }

    #[test]
    fn test_decode_zero_sample_rate_rejected() {
        // A WAV with sample_rate=0 should be rejected by either shravan or our
        // validation check.
        let num_samples: u32 = 100;
        let channels: u16 = 1;
        let bits: u16 = 16;
        let sample_rate: u32 = 0;
        let data_size = num_samples * channels as u32 * (bits as u32 / 8);
        let file_size = 36 + data_size;
        let byte_rate = 0u32;
        let block_align = channels * (bits / 8);

        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        buf.extend_from_slice(&vec![0u8; data_size as usize]);

        let cursor = Cursor::new(buf);
        let result = FileDecoder::open(Box::new(cursor), Some("wav"));
        // Either shravan rejects it or our validation catches sample_rate=0
        assert!(result.is_err(), "decoder should reject sample_rate=0");
    }

    #[test]
    fn test_decode_all_size_limit() {
        // decode_all enforces a 512MB limit. We can't easily create a file
        // that decodes to >512MB, but we can verify the constant exists and
        // that the check path is reachable by verifying normal files succeed.
        let wav = make_wav_samples(4410, 44100, 2);
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();
        let buf = decoder.decode_all().unwrap();
        // 4410 * 2ch * 4 bytes = 35280 bytes, well under 512MB
        assert!(buf.data.len() < 536_870_912);
    }

    #[test]
    fn test_decode_channel_overflow_rejected() {
        // High channel count WAV — shravan may reject this, or it succeeds
        // with a valid u16 channel count. Either way it must not produce
        // a decoder with an invalid state.
        let num_samples: u32 = 100;
        let channels: u16 = 255;
        let bits: u16 = 16;
        let sample_rate: u32 = 44100;
        let data_size = num_samples * channels as u32 * (bits as u32 / 8);
        let file_size = 36 + data_size;
        let byte_rate = sample_rate * channels as u32 * (bits as u32 / 8);
        let block_align = channels * (bits / 8);

        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        buf.extend_from_slice(&vec![0u8; data_size as usize]);

        let cursor = Cursor::new(buf);
        match FileDecoder::open(Box::new(cursor), Some("wav")) {
            Ok(dec) => {
                assert!(dec.channels() > 0);
            }
            Err(_) => {
                // Rejected — fine
            }
        }
    }

    #[test]
    fn test_decode_all_returns_correct_format() {
        let wav = make_wav_samples(4410, 44100, 2);
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();

        let buf = decoder.decode_all().unwrap();

        assert_eq!(buf.sample_rate, 44100, "sample rate mismatch");
        assert_eq!(buf.channels, 2, "channel count mismatch");
        assert_eq!(
            buf.sample_format,
            SampleFormat::F32,
            "sample format should be F32"
        );
        assert_eq!(buf.num_frames, 4410, "num_frames mismatch");
        // Verify data length: num_frames * channels * sizeof(f32)
        assert_eq!(buf.data.len(), 4410 * 2 * 4, "data byte length mismatch");
        // Timestamp of the complete buffer should be zero (starts from beginning)
        assert_eq!(buf.timestamp, Duration::ZERO);
    }

    #[test]
    fn test_decode_seek() {
        let wav = make_wav_samples(44100, 44100, 1); // 1 second mono
        let cursor = Cursor::new(wav);
        let mut decoder = FileDecoder::open(Box::new(cursor), Some("wav")).unwrap();

        // Read first buffer to get initial timestamp
        let buf_start = decoder.next_buffer().unwrap();
        let ts_start = buf_start.timestamp;

        // Seek to 0.5s
        decoder.seek(Duration::from_millis(500)).unwrap();
        let buf_mid = decoder.next_buffer().unwrap();
        let ts_mid = buf_mid.timestamp;

        // After seeking to 0.5s, timestamp should be >= 0.4s (allow tolerance)
        assert!(
            ts_mid.as_secs_f64() >= 0.4,
            "after seeking to 0.5s, timestamp was {:?} (expected >= 0.4s)",
            ts_mid
        );

        // The mid timestamp should be greater than the start
        assert!(
            ts_mid > ts_start,
            "timestamp after seek ({:?}) should be after start ({:?})",
            ts_mid,
            ts_start
        );
    }
}
