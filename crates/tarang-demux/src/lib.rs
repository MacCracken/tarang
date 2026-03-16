//! tarang-demux — Container demuxing for the Tarang media framework
//!
//! Pure Rust container parsers for MP4, MKV/WebM, OGG, WAV, and FLAC.
//! Extracts stream metadata and produces raw codec packets for downstream decoders.

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

use bytes::Bytes;
use std::time::Duration;
use tarang_core::{ContainerFormat, Result, TarangError};

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
    fn probe(&mut self) -> Result<tarang_core::MediaInfo>;

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
}
