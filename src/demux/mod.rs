//! tarang-demux — Container demuxing for the Tarang media framework
//!
//! Pure Rust container parsers for MP4, MKV/WebM, OGG, WAV, and FLAC.
//! Extracts stream metadata and produces raw codec packets for downstream decoders.

pub mod ebml;
mod mkv;
mod mp4;
mod mux;
mod ogg;
mod wav;

pub use mkv::MkvDemuxer;
pub use mp4::Mp4Demuxer;
pub use mux::{MkvMuxer, Mp4Muxer, MuxConfig, Muxer, OggMuxer, WavMuxer};
pub use ogg::OggDemuxer;
pub use wav::WavDemuxer;

use crate::core::{ContainerFormat, Result, TarangError};
use bytes::Bytes;
use std::time::Duration;

/// A raw packet extracted from a container
#[derive(Debug, Clone)]
pub struct Packet {
    pub stream_index: usize,
    pub data: Bytes,
    pub timestamp: Duration,
    pub duration: Option<Duration>,
    pub is_keyframe: bool,
}

/// Trait for container demuxers
pub trait Demuxer {
    /// Probe the container and return media info
    fn probe(&mut self) -> Result<crate::core::MediaInfo>;

    /// Read the next packet from the container
    fn next_packet(&mut self) -> Result<Packet>;

    /// Seek to a timestamp
    fn seek(&mut self, timestamp: Duration) -> Result<()>;
}

/// Detect container format from magic bytes
pub fn detect_format(header: &[u8]) -> Result<ContainerFormat> {
    ContainerFormat::from_magic(header)
        .ok_or_else(|| TarangError::UnsupportedFormat("unknown format".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_wav() {
        let header = b"RIFF\x00\x00\x00\x00WAVE\x00\x00";
        assert_eq!(detect_format(header).unwrap(), ContainerFormat::Wav);
    }

    #[test]
    fn detect_ogg() {
        let header = b"OggS\x00\x02\x00\x00\x00\x00\x00\x00";
        assert_eq!(detect_format(header).unwrap(), ContainerFormat::Ogg);
    }

    #[test]
    fn detect_unknown() {
        let bytes = [0u8; 12];
        assert!(detect_format(&bytes).is_err());
    }

    #[test]
    fn packet_creation() {
        let packet = Packet {
            stream_index: 1,
            data: Bytes::from(vec![1, 2, 3]),
            timestamp: Duration::from_millis(100),
            duration: Some(Duration::from_millis(33)),
            is_keyframe: false,
        };
        assert_eq!(packet.stream_index, 1);
        assert_eq!(packet.data.len(), 3);
        assert!(!packet.is_keyframe);
    }

    #[test]
    fn detect_mp4() {
        let header = b"\x00\x00\x00\x20ftypisom\x00\x00\x00\x00";
        assert_eq!(detect_format(header).unwrap(), ContainerFormat::Mp4);
    }

    #[test]
    fn detect_flac() {
        let header = b"fLaC\x00\x00\x00\x22\x00\x00\x00\x00";
        assert_eq!(detect_format(header).unwrap(), ContainerFormat::Flac);
    }

    #[test]
    fn detect_mkv() {
        let header = &[
            0x1A, 0x45, 0xDF, 0xA3, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x23,
        ];
        assert_eq!(detect_format(header).unwrap(), ContainerFormat::Mkv);
    }

    #[test]
    fn detect_mp3_id3() {
        let header = b"ID3\x04\x00\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(detect_format(header).unwrap(), ContainerFormat::Mp3);
    }

    #[test]
    fn detect_mp3_sync() {
        let header = &[
            0xFF, 0xFB, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(detect_format(header).unwrap(), ContainerFormat::Mp3);
    }

    #[test]
    fn detect_avi() {
        let header = b"RIFF\x00\x00\x00\x00AVI \x00\x00";
        assert_eq!(detect_format(header).unwrap(), ContainerFormat::Avi);
    }

    #[test]
    fn detect_too_short() {
        assert!(detect_format(b"RIFF").is_err());
    }

    #[test]
    fn packet_keyframe() {
        let packet = Packet {
            stream_index: 0,
            data: Bytes::from(vec![0xFF; 1024]),
            timestamp: Duration::ZERO,
            duration: None,
            is_keyframe: true,
        };
        assert!(packet.is_keyframe);
        assert_eq!(packet.duration, None);
    }

    // -----------------------------------------------------------------------
    // Integration tests: full probe → next_packet → seek flow
    // -----------------------------------------------------------------------

    #[test]
    fn integration_wav_probe_read_seek_read() {
        use std::io::Cursor;

        // Build a WAV in memory via WavMuxer
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: crate::core::AudioCodec::Pcm,
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = WavMuxer::new(&mut buf, config);
        mux.write_header().unwrap();
        // Write 1 second of silence: 44100 samples * 2 channels * 2 bytes = 176400 bytes
        let pcm_data = vec![0u8; 176400];
        mux.write_packet(&pcm_data).unwrap();
        mux.finalize().unwrap();

        let data = buf.into_inner();
        let cursor = Cursor::new(data);
        let mut demuxer = WavDemuxer::new(cursor);

        // Probe
        let info = demuxer.probe().unwrap();
        assert_eq!(info.format, ContainerFormat::Wav);
        assert!(info.has_audio());
        let duration = info.duration.unwrap();
        assert!((duration.as_secs_f64() - 1.0).abs() < 0.01);

        // Read first packet
        let p1 = demuxer.next_packet().unwrap();
        assert_eq!(p1.stream_index, 0);
        assert!(p1.is_keyframe);
        let first_ts = p1.timestamp;

        // Read a few more packets to advance
        let mut last_ts = first_ts;
        for _ in 0..5 {
            match demuxer.next_packet() {
                Ok(p) => last_ts = p.timestamp,
                Err(_) => break,
            }
        }
        assert!(last_ts >= first_ts);

        // Seek back to start
        demuxer.seek(Duration::ZERO).unwrap();
        let p_after_seek = demuxer.next_packet().unwrap();
        assert_eq!(p_after_seek.timestamp, Duration::ZERO);

        // Seek to middle
        demuxer.seek(Duration::from_millis(500)).unwrap();
        let p_mid = demuxer.next_packet().unwrap();
        assert!(p_mid.timestamp.as_millis() >= 490);
    }

    #[test]
    fn integration_ogg_mux_demux_roundtrip() {
        use std::io::Cursor;

        // Mux an OGG/Opus file with multiple packets
        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: crate::core::AudioCodec::Opus,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
        };
        let mut mux = OggMuxer::new(&mut buf, config).unwrap();
        mux.write_header().unwrap();

        let packet_data = vec![0xFCu8; 64];
        for _ in 0..5 {
            mux.write_packet(&packet_data).unwrap();
        }
        mux.finalize().unwrap();

        // Now demux it
        let data = buf.into_inner();
        let cursor = Cursor::new(data);
        let mut demuxer = OggDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, ContainerFormat::Ogg);
        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].codec, crate::core::AudioCodec::Opus);
        assert_eq!(audio[0].channels, 2);
        assert_eq!(audio[0].sample_rate, 48000);

        // Read packets back and verify data matches
        let mut count = 0;
        loop {
            match demuxer.next_packet() {
                Ok(p) => {
                    assert_eq!(p.data.len(), 64);
                    assert_eq!(p.stream_index, 0);
                    count += 1;
                }
                Err(TarangError::EndOfStream) | Err(TarangError::DemuxError(_)) => break,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
        assert!(count >= 1, "should read at least one data packet");
    }

    #[test]
    fn integration_ogg_vorbis_mux_demux_roundtrip() {
        use std::io::Cursor;

        let mut buf = Cursor::new(Vec::new());
        let config = MuxConfig {
            codec: crate::core::AudioCodec::Vorbis,
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };
        let mut mux = OggMuxer::new(&mut buf, config).unwrap();
        mux.write_header().unwrap();

        let packet_data = vec![0x42u8; 128];
        for _ in 0..3 {
            mux.write_packet(&packet_data).unwrap();
        }
        mux.finalize().unwrap();

        let data = buf.into_inner();
        let cursor = Cursor::new(data);
        let mut demuxer = OggDemuxer::new(cursor);
        let info = demuxer.probe().unwrap();

        assert_eq!(info.format, ContainerFormat::Ogg);
        let audio = info.audio_streams().collect::<Vec<_>>();
        assert_eq!(audio[0].codec, crate::core::AudioCodec::Vorbis);
        assert_eq!(audio[0].sample_rate, 44100);
        assert_eq!(audio[0].channels, 1);
    }

    #[test]
    fn detect_format_all_supported_magic() {
        // WAV
        let wav = b"RIFF\x00\x00\x00\x00WAVE\x00\x00";
        assert_eq!(detect_format(wav).unwrap(), ContainerFormat::Wav);

        // OGG
        let ogg = b"OggS\x00\x02\x00\x00\x00\x00\x00\x00";
        assert_eq!(detect_format(ogg).unwrap(), ContainerFormat::Ogg);

        // MP4 (ftyp)
        let mp4 = b"\x00\x00\x00\x20ftypisom\x00\x00\x00\x00";
        assert_eq!(detect_format(mp4).unwrap(), ContainerFormat::Mp4);

        // FLAC
        let flac = b"fLaC\x00\x00\x00\x22\x00\x00\x00\x00";
        assert_eq!(detect_format(flac).unwrap(), ContainerFormat::Flac);

        // MKV (EBML header)
        let mkv = &[
            0x1A, 0x45, 0xDF, 0xA3, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x23,
        ];
        assert_eq!(detect_format(mkv).unwrap(), ContainerFormat::Mkv);

        // MP3 ID3
        let mp3_id3 = b"ID3\x04\x00\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(detect_format(mp3_id3).unwrap(), ContainerFormat::Mp3);

        // MP3 sync
        let mp3_sync = &[
            0xFF, 0xFB, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(detect_format(mp3_sync).unwrap(), ContainerFormat::Mp3);

        // AVI
        let avi = b"RIFF\x00\x00\x00\x00AVI \x00\x00";
        assert_eq!(detect_format(avi).unwrap(), ContainerFormat::Avi);

        // Unknown
        assert!(detect_format(&[0u8; 12]).is_err());

        // Too short
        assert!(detect_format(b"RIF").is_err());
    }
}
