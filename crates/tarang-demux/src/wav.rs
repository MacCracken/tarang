//! WAV container demuxer (pure Rust)

use bytes::Bytes;
use std::io::{Read, Seek};
use std::time::Duration;
use tarang_core::{
    AudioCodec, AudioStreamInfo, ContainerFormat, MediaInfo, Result, SampleFormat, StreamInfo,
    TarangError,
};
use uuid::Uuid;

use crate::{Demuxer, Packet};

/// WAV container demuxer
pub struct WavDemuxer<R: Read + Seek> {
    reader: R,
    info: Option<MediaInfo>,
    data_offset: u64,
    data_size: u64,
    bytes_read: u64,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
}

impl<R: Read + Seek> WavDemuxer<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            info: None,
            data_offset: 0,
            data_size: 0,
            bytes_read: 0,
            sample_rate: 0,
            channels: 0,
            bits_per_sample: 0,
        }
    }
}

impl<R: Read + Seek> Demuxer for WavDemuxer<R> {
    fn probe(&mut self) -> Result<MediaInfo> {
        let mut header = [0u8; 12];
        self.reader
            .read_exact(&mut header)
            .map_err(|e| TarangError::DemuxError(format!("failed to read WAV header: {e}")))?;

        if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
            return Err(TarangError::UnsupportedFormat("not a WAV file".to_string()));
        }

        // Parse chunks
        loop {
            let mut chunk_header = [0u8; 8];
            if self.reader.read_exact(&mut chunk_header).is_err() {
                break;
            }

            let chunk_id = &chunk_header[0..4];
            let chunk_size = u32::from_le_bytes([
                chunk_header[4],
                chunk_header[5],
                chunk_header[6],
                chunk_header[7],
            ]) as u64;

            if chunk_id == b"fmt " {
                let mut fmt = [0u8; 16];
                self.reader.read_exact(&mut fmt).map_err(|e| {
                    TarangError::DemuxError(format!("failed to read fmt chunk: {e}"))
                })?;

                self.channels = u16::from_le_bytes([fmt[2], fmt[3]]);
                self.sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                self.bits_per_sample = u16::from_le_bytes([fmt[14], fmt[15]]);

                // Skip any extra fmt bytes
                if chunk_size > 16 {
                    self.reader
                        .seek(std::io::SeekFrom::Current((chunk_size - 16) as i64))
                        .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
                }
            } else if chunk_id == b"data" {
                self.data_offset = self
                    .reader
                    .stream_position()
                    .map_err(|e| TarangError::DemuxError(format!("failed to get position: {e}")))?;
                self.data_size = chunk_size;
                break;
            } else {
                self.reader
                    .seek(std::io::SeekFrom::Current(chunk_size as i64))
                    .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
            }
        }

        if self.sample_rate == 0 {
            return Err(TarangError::DemuxError("no fmt chunk found".to_string()));
        }

        let bytes_per_sample = self.bits_per_sample as u64 / 8;
        let total_samples = if bytes_per_sample > 0 && self.channels > 0 {
            self.data_size / (bytes_per_sample * self.channels as u64)
        } else {
            0
        };
        let duration = if self.sample_rate > 0 {
            Some(Duration::from_secs_f64(
                total_samples as f64 / self.sample_rate as f64,
            ))
        } else {
            None
        };

        let sample_format = match self.bits_per_sample {
            16 => SampleFormat::I16,
            32 => SampleFormat::I32,
            _ => SampleFormat::I16,
        };

        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Wav,
            streams: vec![StreamInfo::Audio(AudioStreamInfo {
                codec: AudioCodec::Pcm,
                sample_rate: self.sample_rate,
                channels: self.channels,
                sample_format,
                bitrate: self
                    .sample_rate
                    .checked_mul(self.channels as u32)
                    .and_then(|v| v.checked_mul(self.bits_per_sample as u32)),
                duration,
            })],
            duration,
            file_size: None,
            title: None,
            artist: None,
            album: None,
        };

        self.info = Some(info.clone());
        Ok(info)
    }

    fn next_packet(&mut self) -> Result<Packet> {
        if self.bytes_read >= self.data_size {
            return Err(TarangError::EndOfStream);
        }

        let chunk_size = 4096.min((self.data_size - self.bytes_read) as usize);
        if chunk_size == 0 {
            return Err(TarangError::EndOfStream);
        }

        let mut buf = vec![0u8; chunk_size];
        let n = self
            .reader
            .read(&mut buf)
            .map_err(|e| TarangError::DemuxError(format!("read error: {e}")))?;
        if n == 0 {
            return Err(TarangError::EndOfStream);
        }
        buf.truncate(n);

        let bytes_per_sample = (self.bits_per_sample as u64 / 8) * self.channels as u64;
        let timestamp = if bytes_per_sample > 0 && self.sample_rate > 0 {
            let samples = self.bytes_read / bytes_per_sample;
            Duration::from_secs_f64(samples as f64 / self.sample_rate as f64)
        } else {
            Duration::ZERO
        };

        self.bytes_read += n as u64;

        Ok(Packet {
            stream_index: 0,
            data: Bytes::from(buf),
            timestamp,
            duration: None,
            is_keyframe: true,
        })
    }

    fn seek(&mut self, timestamp: Duration) -> Result<()> {
        let bytes_per_sample = (self.bits_per_sample as u64 / 8) * self.channels as u64;
        let target_sample = (timestamp.as_secs_f64() * self.sample_rate as f64) as u64;
        let byte_offset = (target_sample * bytes_per_sample).min(self.data_size);

        self.reader
            .seek(std::io::SeekFrom::Start(self.data_offset + byte_offset))
            .map_err(|e| TarangError::DemuxError(format!("seek error: {e}")))?;
        self.bytes_read = byte_offset;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn make_wav(samples: u32, sample_rate: u32, channels: u16, bits: u16) -> Vec<u8> {
        let data_size = samples * channels as u32 * (bits as u32 / 8);
        let file_size = 36 + data_size;
        let byte_rate = sample_rate * channels as u32 * (bits as u32 / 8);
        let block_align = channels * (bits / 8);

        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        // fmt chunk
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits.to_le_bytes());
        // data chunk
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        buf.extend_from_slice(&vec![0u8; data_size as usize]);
        buf
    }

    #[test]
    fn wav_probe() {
        let wav = make_wav(44100, 44100, 2, 16);
        let cursor = Cursor::new(wav);
        let mut demuxer = WavDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, ContainerFormat::Wav);
        assert_eq!(info.streams.len(), 1);
        assert!(info.has_audio());
        assert!(!info.has_video());

        let audio = info.audio_streams();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].codec, AudioCodec::Pcm);
        assert_eq!(audio[0].sample_rate, 44100);
        assert_eq!(audio[0].channels, 2);
    }

    #[test]
    fn wav_probe_mono() {
        let wav = make_wav(48000, 48000, 1, 16);
        let cursor = Cursor::new(wav);
        let mut demuxer = WavDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let audio = info.audio_streams();
        assert_eq!(audio[0].channels, 1);
        assert_eq!(audio[0].sample_rate, 48000);
    }

    #[test]
    fn wav_duration() {
        let wav = make_wav(44100, 44100, 2, 16);
        let cursor = Cursor::new(wav);
        let mut demuxer = WavDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        let duration = info.duration.unwrap();
        assert!((duration.as_secs_f64() - 1.0).abs() < 0.01);
    }

    #[test]
    fn wav_read_packets() {
        let wav = make_wav(1000, 44100, 2, 16);
        let cursor = Cursor::new(wav);
        let mut demuxer = WavDemuxer::new(cursor);
        demuxer.probe().unwrap();

        let packet = demuxer.next_packet().unwrap();
        assert_eq!(packet.stream_index, 0);
        assert!(packet.is_keyframe);
        assert!(!packet.data.is_empty());
    }

    #[test]
    fn wav_end_of_stream() {
        let wav = make_wav(10, 44100, 1, 16);
        let cursor = Cursor::new(wav);
        let mut demuxer = WavDemuxer::new(cursor);
        demuxer.probe().unwrap();

        // Read all packets
        loop {
            match demuxer.next_packet() {
                Ok(_) => continue,
                Err(TarangError::EndOfStream) => break,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
    }

    #[test]
    fn wav_seek() {
        let wav = make_wav(44100, 44100, 2, 16);
        let cursor = Cursor::new(wav);
        let mut demuxer = WavDemuxer::new(cursor);
        demuxer.probe().unwrap();

        demuxer.seek(Duration::from_millis(500)).unwrap();
        let packet = demuxer.next_packet().unwrap();
        assert!(packet.timestamp.as_millis() >= 490);
    }

    #[test]
    fn wav_invalid_header() {
        let cursor = Cursor::new(vec![0u8; 100]);
        let mut demuxer = WavDemuxer::new(cursor);
        assert!(demuxer.probe().is_err());
    }
}
