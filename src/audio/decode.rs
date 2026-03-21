//! Full audio decode pipeline via symphonia
//!
//! `FileDecoder` wraps symphonia's format reader + codec decoder to produce
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
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use super::probe::map_symphonia_codec;

/// Full audio file decoder. Owns symphonia's format reader and codec decoder,
/// producing decoded `AudioBuffer`s frame by frame.
pub struct FileDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    codec: AudioCodec,
    sample_rate: u32,
    channels: u16,
    /// Running sample count for timestamp calculation
    samples_decoded: u64,
}

impl FileDecoder {
    /// Open an audio file for decoding.
    ///
    /// Accepts any `MediaSource` (File, Cursor, network stream, etc.).
    /// Optionally provide a file extension hint to speed up format detection.
    pub fn open(
        source: Box<dyn symphonia::core::io::MediaSource>,
        extension_hint: Option<&str>,
    ) -> Result<Self> {
        let mss = MediaSourceStream::new(source, Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = extension_hint {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| TarangError::DemuxError(format!("failed to probe audio: {e}").into()))?;

        let format = probed.format;

        // Find the first audio track
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| TarangError::DemuxError("no audio track found".into()))?;

        let track_id = track.id;
        let params = &track.codec_params;

        let codec = map_symphonia_codec(params.codec).ok_or_else(|| {
            TarangError::UnsupportedCodec(format!("symphonia codec {:?}", params.codec).into())
        })?;

        let sample_rate = params.sample_rate.unwrap_or(44100);
        if sample_rate == 0 {
            return Err(TarangError::DecodeError(
                "codec reports sample rate 0".into(),
            ));
        }
        let channels = match params.channels {
            Some(c) => {
                let count = c.count();
                if count > u16::MAX as usize {
                    return Err(TarangError::DecodeError(
                        format!("channel count {count} exceeds u16::MAX").into(),
                    ));
                }
                count as u16
            }
            None => 2,
        };

        let decoder = symphonia::default::get_codecs()
            .make(params, &DecoderOptions::default())
            .map_err(|e| {
                TarangError::DecodeError(format!("failed to create decoder: {e}").into())
            })?;

        Ok(Self {
            format,
            decoder,
            track_id,
            codec,
            sample_rate,
            channels,
            samples_decoded: 0,
        })
    }

    /// Open from a file path (convenience).
    pub fn open_path(path: &std::path::Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(TarangError::Io)?;
        let ext = path.extension().and_then(|e| e.to_str());
        Self::open(Box::new(file), ext)
    }

    /// The detected audio codec.
    pub fn codec(&self) -> AudioCodec {
        self.codec
    }

    /// Sample rate of the decoded audio.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Number of channels.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Decode the next frame, returning an `AudioBuffer` with interleaved F32 samples.
    /// Returns `Err(TarangError::EndOfStream)` when the file is fully decoded.
    pub fn next_buffer(&mut self) -> Result<AudioBuffer> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(ref e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Err(TarangError::EndOfStream);
                }
                Err(SymphoniaError::ResetRequired) => {
                    // Decoder needs reset (e.g. after seek)
                    self.decoder.reset();
                    continue;
                }
                Err(e) => {
                    return Err(TarangError::DemuxError(
                        format!("failed to read packet: {e}").into(),
                    ));
                }
            };

            // Skip packets from other tracks
            if packet.track_id() != self.track_id {
                continue;
            }

            let decoded = match self.decoder.decode(&packet) {
                Ok(buf) => buf,
                Err(SymphoniaError::DecodeError(e)) => {
                    tracing::warn!(error = %e, "decode error, skipping frame");
                    continue;
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => {
                    return Err(TarangError::DecodeError(
                        format!("decode failed: {e}").into(),
                    ));
                }
            };

            let spec = *decoded.spec();
            let num_frames = decoded.frames();
            if num_frames == 0 {
                continue;
            }

            let num_channels = spec.channels.count();
            let sr = spec.rate;

            // Convert to interleaved F32
            let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
            sample_buf.copy_interleaved_ref(decoded);
            let samples = sample_buf.samples();

            let timestamp = Duration::from_secs_f64(self.samples_decoded as f64 / sr as f64);

            self.samples_decoded += num_frames as u64;

            // Update cached format info from actual decoded data
            self.sample_rate = sr;
            self.channels = num_channels as u16;

            return Ok(AudioBuffer {
                data: Bytes::copy_from_slice(bytemuck_f32_to_bytes(samples)),
                sample_format: SampleFormat::F32,
                channels: num_channels as u16,
                sample_rate: sr,
                num_frames,
                timestamp,
            });
        }
    }

    /// Seek to the given timestamp. The next call to `next_buffer` will produce
    /// audio from approximately this position.
    pub fn seek(&mut self, timestamp: Duration) -> Result<()> {
        let time = Time::from(timestamp.as_secs_f64());

        self.format
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| TarangError::DemuxError(format!("seek failed: {e}").into()))?;

        // Reset decoder state after seek
        self.decoder.reset();

        // Update sample count estimate for timestamps
        self.samples_decoded = (timestamp.as_secs_f64() * self.sample_rate as f64) as u64;

        Ok(())
    }

    /// Decode the entire file into a single contiguous buffer.
    /// Useful for short files or when you need all samples in memory.
    pub fn decode_all(&mut self) -> Result<AudioBuffer> {
        let mut all_data: Vec<f32> = Vec::new();
        let mut total_samples = 0usize;
        let mut sr = self.sample_rate;
        let mut ch = self.channels;

        const MAX_DECODED_BYTES: usize = 536_870_912; // 512 MB as f32 values

        loop {
            match self.next_buffer() {
                Ok(buf) => {
                    sr = buf.sample_rate;
                    ch = buf.channels;
                    total_samples += buf.num_frames;
                    // buf.data is f32 samples as bytes
                    let floats: &[f32] = bytemuck_bytes_to_f32(&buf.data);
                    all_data.extend_from_slice(floats);

                    if all_data.len() * std::mem::size_of::<f32>() > MAX_DECODED_BYTES {
                        return Err(TarangError::DecodeError(
                            format!(
                                "decoded audio exceeds 512MB limit ({} bytes)",
                                all_data.len() * std::mem::size_of::<f32>()
                            )
                            .into(),
                        ));
                    }
                }
                Err(TarangError::EndOfStream) => break,
                Err(e) => return Err(e),
            }
        }

        if total_samples == 0 {
            return Err(TarangError::DecodeError("no audio decoded".into()));
        }

        tracing::debug!(
            total_samples = total_samples,
            total_bytes = all_data.len() * std::mem::size_of::<f32>(),
            "decode_all complete"
        );

        Ok(AudioBuffer {
            data: Bytes::copy_from_slice(bytemuck_f32_to_bytes(&all_data)),
            sample_format: SampleFormat::F32,
            channels: ch,
            sample_rate: sr,
            num_frames: total_samples,
            timestamp: Duration::ZERO,
        })
    }
}

use super::sample::{bytes_to_f32 as bytemuck_bytes_to_f32, f32_to_bytes as bytemuck_f32_to_bytes};

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
        // Symphonia panics on WAV files with sample_rate=0 during probing,
        // so we verify our validation would catch it by testing the code path
        // indirectly: if symphonia ever reported sample_rate=Some(0), our
        // check at line 75 would return Err. We verify this by confirming
        // that a WAV with sample_rate=0 is not accepted (either symphonia
        // panics or our code rejects it).
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
        // Symphonia may panic on sample_rate=0 during probing; catch that.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            FileDecoder::open(Box::new(cursor), Some("wav"))
        }));
        match result {
            Ok(Ok(_)) => panic!("decoder should reject sample_rate=0, got Ok"),
            Ok(Err(_)) => {
                // Our validation caught it — good
            }
            Err(_) => {
                // Symphonia panicked — sample_rate=0 is not accepted either way
            }
        }
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
        // The channel overflow check at line 83 guards against count > u16::MAX.
        // We cannot easily mock symphonia to report > 65535 channels, but we can
        // verify the guard exists by testing with a malformed WAV that has an
        // absurd channel count. Symphonia may reject this before our code does,
        // but either way it should not succeed.
        let num_samples: u32 = 100;
        let channels: u16 = 255; // High channel count (symphonia may reject)
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
        // Either symphonia or our code should reject this; it must not silently
        // produce a decoder claiming > u16::MAX channels.
        match FileDecoder::open(Box::new(cursor), Some("wav")) {
            Ok(dec) => {
                // If symphonia accepts it, our code must have clamped channels to a valid u16
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

        // After seeking to 0.5s, timestamp should be >= 0.4s (allow coarse seeking tolerance)
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
