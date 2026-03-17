//! tarang-core — Core types for the Tarang media framework
//!
//! Defines codecs, container formats, media buffers, stream metadata,
//! and pipeline primitives used across all Tarang crates.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// Supported audio codecs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioCodec {
    Pcm,
    Mp3,
    Aac,
    Flac,
    Vorbis,
    Opus,
    Alac,
    Wma,
}

impl std::fmt::Display for AudioCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pcm => write!(f, "PCM"),
            Self::Mp3 => write!(f, "MP3"),
            Self::Aac => write!(f, "AAC"),
            Self::Flac => write!(f, "FLAC"),
            Self::Vorbis => write!(f, "Vorbis"),
            Self::Opus => write!(f, "Opus"),
            Self::Alac => write!(f, "ALAC"),
            Self::Wma => write!(f, "WMA"),
        }
    }
}

/// Supported video codecs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VideoCodec {
    H264,
    H265,
    Vp8,
    Vp9,
    Av1,
    Theora,
}

impl std::fmt::Display for VideoCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::H264 => write!(f, "H.264"),
            Self::H265 => write!(f, "H.265"),
            Self::Vp8 => write!(f, "VP8"),
            Self::Vp9 => write!(f, "VP9"),
            Self::Av1 => write!(f, "AV1"),
            Self::Theora => write!(f, "Theora"),
        }
    }
}

/// Supported container formats
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContainerFormat {
    Mp4,
    Mkv,
    WebM,
    Ogg,
    Wav,
    Flac,
    Mp3,
    Avi,
}

impl ContainerFormat {
    /// Common file extensions for this container
    pub fn extensions(&self) -> &[&str] {
        match self {
            Self::Mp4 => &["mp4", "m4a", "m4v"],
            Self::Mkv => &["mkv", "mka"],
            Self::WebM => &["webm"],
            Self::Ogg => &["ogg", "oga", "ogv"],
            Self::Wav => &["wav"],
            Self::Flac => &["flac"],
            Self::Mp3 => &["mp3"],
            Self::Avi => &["avi"],
        }
    }

    /// Detect container format from magic bytes
    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 12 {
            return None;
        }
        // ftyp box for MP4
        if &bytes[4..8] == b"ftyp" {
            return Some(Self::Mp4);
        }
        // EBML header for MKV/WebM — check DocType to distinguish
        if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            // Search for "webm" DocType string in the first bytes
            if let Some(pos) = bytes.windows(4).position(|w| w == b"webm")
                && pos < 64
            {
                return Some(Self::WebM);
            }
            return Some(Self::Mkv);
        }
        // OggS
        if bytes.starts_with(b"OggS") {
            return Some(Self::Ogg);
        }
        // RIFF....WAVE
        if bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
            return Some(Self::Wav);
        }
        // fLaC
        if bytes.starts_with(b"fLaC") {
            return Some(Self::Flac);
        }
        // ID3 or sync word for MP3
        if bytes.starts_with(b"ID3")
            || (bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0)
        {
            return Some(Self::Mp3);
        }
        // RIFF....AVI
        if bytes.starts_with(b"RIFF") && &bytes[8..12] == b"AVI " {
            return Some(Self::Avi);
        }
        None
    }
}

impl std::fmt::Display for ContainerFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mp4 => write!(f, "MP4"),
            Self::Mkv => write!(f, "Matroska"),
            Self::WebM => write!(f, "WebM"),
            Self::Ogg => write!(f, "OGG"),
            Self::Wav => write!(f, "WAV"),
            Self::Flac => write!(f, "FLAC"),
            Self::Mp3 => write!(f, "MP3"),
            Self::Avi => write!(f, "AVI"),
        }
    }
}

/// Sample format for decoded audio
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SampleFormat {
    I16,
    I32,
    F32,
    F64,
}

impl SampleFormat {
    pub fn bytes_per_sample(&self) -> usize {
        match self {
            Self::I16 => 2,
            Self::I32 => 4,
            Self::F32 => 4,
            Self::F64 => 8,
        }
    }
}

/// Pixel format for decoded video frames
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PixelFormat {
    Yuv420p,
    Yuv422p,
    Yuv444p,
    Rgb24,
    Rgba32,
    Nv12,
}

/// Audio stream metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
    pub bitrate: Option<u32>,
    pub duration: Option<Duration>,
}

/// Video stream metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoStreamInfo {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub frame_rate: f64,
    pub bitrate: Option<u32>,
    pub duration: Option<Duration>,
}

/// A stream within a media container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamInfo {
    Audio(AudioStreamInfo),
    Video(VideoStreamInfo),
    Subtitle { language: Option<String> },
}

/// Metadata about a media file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInfo {
    pub id: Uuid,
    pub format: ContainerFormat,
    pub streams: Vec<StreamInfo>,
    pub duration: Option<Duration>,
    pub file_size: Option<u64>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

impl MediaInfo {
    pub fn audio_streams(&self) -> Vec<&AudioStreamInfo> {
        self.streams
            .iter()
            .filter_map(|s| match s {
                StreamInfo::Audio(a) => Some(a),
                _ => None,
            })
            .collect()
    }

    pub fn video_streams(&self) -> Vec<&VideoStreamInfo> {
        self.streams
            .iter()
            .filter_map(|s| match s {
                StreamInfo::Video(v) => Some(v),
                _ => None,
            })
            .collect()
    }

    pub fn has_video(&self) -> bool {
        self.streams
            .iter()
            .any(|s| matches!(s, StreamInfo::Video(_)))
    }

    pub fn has_audio(&self) -> bool {
        self.streams
            .iter()
            .any(|s| matches!(s, StreamInfo::Audio(_)))
    }
}

/// A decoded audio buffer
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub data: Bytes,
    pub sample_format: SampleFormat,
    pub channels: u16,
    pub sample_rate: u32,
    pub num_samples: usize,
    pub timestamp: Duration,
}

/// Compute the byte size of a YUV420p frame with the given dimensions.
/// Uses ceiling division for chroma planes (correct for odd sizes).
pub fn yuv420p_frame_size(width: u32, height: u32) -> usize {
    let y_size = (width as usize) * (height as usize);
    let chroma_w = (width as usize).div_ceil(2);
    let chroma_h = (height as usize).div_ceil(2);
    y_size + 2 * chroma_w * chroma_h
}

/// Validate video encoder dimensions (must be non-zero and even).
pub fn validate_video_dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(TarangError::Pipeline(
            "width and height must be non-zero".to_string(),
        ));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(TarangError::Pipeline(format!(
            "dimensions must be even, got {width}x{height}"
        )));
    }
    Ok(())
}

/// A decoded video frame
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub data: Bytes,
    pub pixel_format: PixelFormat,
    pub width: u32,
    pub height: u32,
    pub timestamp: Duration,
}

/// Pipeline status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineState {
    Idle,
    Opening,
    Playing,
    Paused,
    Seeking,
    Error,
    Finished,
}

/// Error types for the Tarang framework
#[derive(Debug, thiserror::Error)]
pub enum TarangError {
    #[error("unsupported codec: {0}")]
    UnsupportedCodec(String),
    #[error("unsupported container format: {0}")]
    UnsupportedFormat(String),
    #[error("decode error: {0}")]
    DecodeError(String),
    #[error("demux error: {0}")]
    DemuxError(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("end of stream")]
    EndOfStream,
    #[error("pipeline error: {0}")]
    Pipeline(String),
    #[error("hardware acceleration error: {0}")]
    HwAccelError(String),
    #[error("AI feature error: {0}")]
    AiError(String),
    #[error("network error: {0}")]
    NetworkError(String),
    #[error("image error: {0}")]
    ImageError(String),
}

pub type Result<T> = std::result::Result<T, TarangError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_codec_display() {
        assert_eq!(AudioCodec::Mp3.to_string(), "MP3");
        assert_eq!(AudioCodec::Flac.to_string(), "FLAC");
        assert_eq!(AudioCodec::Opus.to_string(), "Opus");
        assert_eq!(AudioCodec::Vorbis.to_string(), "Vorbis");
    }

    #[test]
    fn video_codec_display() {
        assert_eq!(VideoCodec::H264.to_string(), "H.264");
        assert_eq!(VideoCodec::Av1.to_string(), "AV1");
        assert_eq!(VideoCodec::Vp9.to_string(), "VP9");
    }

    #[test]
    fn container_format_display() {
        assert_eq!(ContainerFormat::Mp4.to_string(), "MP4");
        assert_eq!(ContainerFormat::Mkv.to_string(), "Matroska");
        assert_eq!(ContainerFormat::WebM.to_string(), "WebM");
    }

    #[test]
    fn container_extensions() {
        assert!(ContainerFormat::Mp4.extensions().contains(&"mp4"));
        assert!(ContainerFormat::Mp4.extensions().contains(&"m4a"));
        assert!(ContainerFormat::Mkv.extensions().contains(&"mkv"));
        assert!(ContainerFormat::Ogg.extensions().contains(&"ogg"));
    }

    #[test]
    fn magic_bytes_mp4() {
        let bytes = b"\x00\x00\x00\x20ftypisom\x00\x00\x00\x00";
        assert_eq!(
            ContainerFormat::from_magic(bytes),
            Some(ContainerFormat::Mp4)
        );
    }

    #[test]
    fn magic_bytes_ogg() {
        let bytes = b"OggS\x00\x02\x00\x00\x00\x00\x00\x00";
        assert_eq!(
            ContainerFormat::from_magic(bytes),
            Some(ContainerFormat::Ogg)
        );
    }

    #[test]
    fn magic_bytes_wav() {
        let bytes = b"RIFF\x00\x00\x00\x00WAVE\x00\x00";
        assert_eq!(
            ContainerFormat::from_magic(bytes),
            Some(ContainerFormat::Wav)
        );
    }

    #[test]
    fn magic_bytes_flac() {
        let bytes = b"fLaC\x00\x00\x00\x22\x00\x00\x00\x00";
        assert_eq!(
            ContainerFormat::from_magic(bytes),
            Some(ContainerFormat::Flac)
        );
    }

    #[test]
    fn magic_bytes_mkv() {
        let bytes = &[
            0x1A, 0x45, 0xDF, 0xA3, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x23,
        ];
        assert_eq!(
            ContainerFormat::from_magic(bytes),
            Some(ContainerFormat::Mkv)
        );
    }

    #[test]
    fn magic_bytes_mp3_id3() {
        let bytes = b"ID3\x04\x00\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(
            ContainerFormat::from_magic(bytes),
            Some(ContainerFormat::Mp3)
        );
    }

    #[test]
    fn magic_bytes_mp3_sync() {
        let bytes = &[
            0xFF, 0xFB, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(
            ContainerFormat::from_magic(bytes),
            Some(ContainerFormat::Mp3)
        );
    }

    #[test]
    fn magic_bytes_avi() {
        let bytes = b"RIFF\x00\x00\x00\x00AVI \x00\x00";
        assert_eq!(
            ContainerFormat::from_magic(bytes),
            Some(ContainerFormat::Avi)
        );
    }

    #[test]
    fn magic_bytes_too_short() {
        assert_eq!(ContainerFormat::from_magic(b"OggS"), None);
    }

    #[test]
    fn magic_bytes_unknown() {
        let bytes = b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        assert_eq!(ContainerFormat::from_magic(bytes), None);
    }

    #[test]
    fn sample_format_sizes() {
        assert_eq!(SampleFormat::I16.bytes_per_sample(), 2);
        assert_eq!(SampleFormat::I32.bytes_per_sample(), 4);
        assert_eq!(SampleFormat::F32.bytes_per_sample(), 4);
        assert_eq!(SampleFormat::F64.bytes_per_sample(), 8);
    }

    #[test]
    fn media_info_stream_queries() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mp4,
            streams: vec![
                StreamInfo::Video(VideoStreamInfo {
                    codec: VideoCodec::H264,
                    width: 1920,
                    height: 1080,
                    pixel_format: PixelFormat::Yuv420p,
                    frame_rate: 30.0,
                    bitrate: Some(5_000_000),
                    duration: Some(Duration::from_secs(120)),
                }),
                StreamInfo::Audio(AudioStreamInfo {
                    codec: AudioCodec::Aac,
                    sample_rate: 48000,
                    channels: 2,
                    sample_format: SampleFormat::F32,
                    bitrate: Some(128_000),
                    duration: Some(Duration::from_secs(120)),
                }),
                StreamInfo::Subtitle {
                    language: Some("en".to_string()),
                },
            ],
            duration: Some(Duration::from_secs(120)),
            file_size: Some(75_000_000),
            title: Some("Test Video".to_string()),
            artist: None,
            album: None,
        };

        assert!(info.has_video());
        assert!(info.has_audio());
        assert_eq!(info.video_streams().len(), 1);
        assert_eq!(info.audio_streams().len(), 1);
        assert_eq!(info.video_streams()[0].width, 1920);
        assert_eq!(info.audio_streams()[0].sample_rate, 48000);
    }

    #[test]
    fn media_info_audio_only() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Flac,
            streams: vec![StreamInfo::Audio(AudioStreamInfo {
                codec: AudioCodec::Flac,
                sample_rate: 44100,
                channels: 2,
                sample_format: SampleFormat::I16,
                bitrate: None,
                duration: Some(Duration::from_secs(300)),
            })],
            duration: Some(Duration::from_secs(300)),
            file_size: Some(30_000_000),
            title: Some("Track 1".to_string()),
            artist: Some("Artist".to_string()),
            album: Some("Album".to_string()),
        };

        assert!(!info.has_video());
        assert!(info.has_audio());
        assert_eq!(info.video_streams().len(), 0);
        assert_eq!(info.audio_streams().len(), 1);
    }

    #[test]
    fn pipeline_state_equality() {
        assert_eq!(PipelineState::Idle, PipelineState::Idle);
        assert_ne!(PipelineState::Playing, PipelineState::Paused);
    }

    #[test]
    fn tarang_error_display() {
        let err = TarangError::UnsupportedCodec("HEVC".to_string());
        assert_eq!(err.to_string(), "unsupported codec: HEVC");

        let err = TarangError::EndOfStream;
        assert_eq!(err.to_string(), "end of stream");
    }

    #[test]
    fn audio_buffer_creation() {
        let buf = AudioBuffer {
            data: Bytes::from(vec![0u8; 4096]),
            sample_format: SampleFormat::F32,
            channels: 2,
            sample_rate: 44100,
            num_samples: 512,
            timestamp: Duration::from_millis(500),
        };
        assert_eq!(buf.num_samples, 512);
        assert_eq!(buf.channels, 2);
        assert_eq!(buf.data.len(), 4096);
    }

    #[test]
    fn video_frame_creation() {
        let frame = VideoFrame {
            data: Bytes::from(vec![0u8; 1920 * 1080 * 3]),
            pixel_format: PixelFormat::Rgb24,
            width: 1920,
            height: 1080,
            timestamp: Duration::from_millis(33),
        };
        assert_eq!(frame.width, 1920);
        assert_eq!(frame.height, 1080);
        assert_eq!(frame.data.len(), 1920 * 1080 * 3);
    }

    #[test]
    fn container_format_serialization() {
        let json = serde_json::to_string(&ContainerFormat::Mp4).unwrap();
        assert_eq!(json, "\"Mp4\"");
        let parsed: ContainerFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ContainerFormat::Mp4);
    }

    #[test]
    fn audio_codec_serialization() {
        let json = serde_json::to_string(&AudioCodec::Opus).unwrap();
        let parsed: AudioCodec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, AudioCodec::Opus);
    }

    #[test]
    fn video_codec_serialization() {
        let json = serde_json::to_string(&VideoCodec::Av1).unwrap();
        let parsed: VideoCodec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, VideoCodec::Av1);
    }

    #[test]
    fn audio_codec_display_all_variants() {
        assert_eq!(AudioCodec::Pcm.to_string(), "PCM");
        assert_eq!(AudioCodec::Aac.to_string(), "AAC");
        assert_eq!(AudioCodec::Alac.to_string(), "ALAC");
        assert_eq!(AudioCodec::Wma.to_string(), "WMA");
    }

    #[test]
    fn video_codec_display_all_variants() {
        assert_eq!(VideoCodec::H265.to_string(), "H.265");
        assert_eq!(VideoCodec::Vp8.to_string(), "VP8");
        assert_eq!(VideoCodec::Theora.to_string(), "Theora");
    }

    #[test]
    fn container_format_display_all_variants() {
        assert_eq!(ContainerFormat::Ogg.to_string(), "OGG");
        assert_eq!(ContainerFormat::Wav.to_string(), "WAV");
        assert_eq!(ContainerFormat::Flac.to_string(), "FLAC");
        assert_eq!(ContainerFormat::Mp3.to_string(), "MP3");
        assert_eq!(ContainerFormat::Avi.to_string(), "AVI");
    }

    #[test]
    fn container_extensions_all_formats() {
        assert!(ContainerFormat::WebM.extensions().contains(&"webm"));
        assert!(ContainerFormat::Wav.extensions().contains(&"wav"));
        assert!(ContainerFormat::Flac.extensions().contains(&"flac"));
        assert!(ContainerFormat::Mp3.extensions().contains(&"mp3"));
        assert!(ContainerFormat::Avi.extensions().contains(&"avi"));
        assert!(ContainerFormat::Ogg.extensions().contains(&"oga"));
        assert!(ContainerFormat::Ogg.extensions().contains(&"ogv"));
        assert!(ContainerFormat::Mp4.extensions().contains(&"m4v"));
        assert!(ContainerFormat::Mkv.extensions().contains(&"mka"));
    }

    #[test]
    fn media_info_no_streams() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mp4,
            streams: Vec::new(),
            duration: None,
            file_size: None,
            title: None,
            artist: None,
            album: None,
        };
        assert!(!info.has_video());
        assert!(!info.has_audio());
        assert!(info.video_streams().is_empty());
        assert!(info.audio_streams().is_empty());
    }

    #[test]
    fn media_info_subtitle_only() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mkv,
            streams: vec![StreamInfo::Subtitle {
                language: Some("fr".to_string()),
            }],
            duration: None,
            file_size: None,
            title: None,
            artist: None,
            album: None,
        };
        assert!(!info.has_video());
        assert!(!info.has_audio());
    }

    #[test]
    fn tarang_error_all_variants_display() {
        assert_eq!(
            TarangError::UnsupportedFormat("X".to_string()).to_string(),
            "unsupported container format: X"
        );
        assert_eq!(
            TarangError::DecodeError("bad".to_string()).to_string(),
            "decode error: bad"
        );
        assert_eq!(
            TarangError::DemuxError("fail".to_string()).to_string(),
            "demux error: fail"
        );
        assert_eq!(
            TarangError::Pipeline("broken".to_string()).to_string(),
            "pipeline error: broken"
        );
        assert_eq!(
            TarangError::HwAccelError("no gpu".to_string()).to_string(),
            "hardware acceleration error: no gpu"
        );
        assert_eq!(
            TarangError::AiError("oops".to_string()).to_string(),
            "AI feature error: oops"
        );
        assert_eq!(
            TarangError::NetworkError("timeout".to_string()).to_string(),
            "network error: timeout"
        );
        assert_eq!(
            TarangError::ImageError("corrupt".to_string()).to_string(),
            "image error: corrupt"
        );
    }

    #[test]
    fn pipeline_state_serialization() {
        for state in [
            PipelineState::Idle,
            PipelineState::Opening,
            PipelineState::Playing,
            PipelineState::Paused,
            PipelineState::Seeking,
            PipelineState::Error,
            PipelineState::Finished,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: PipelineState = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, state);
        }
    }

    #[test]
    fn sample_format_serialization() {
        for fmt in [
            SampleFormat::I16,
            SampleFormat::I32,
            SampleFormat::F32,
            SampleFormat::F64,
        ] {
            let json = serde_json::to_string(&fmt).unwrap();
            let parsed: SampleFormat = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, fmt);
        }
    }

    #[test]
    fn pixel_format_serialization() {
        for fmt in [
            PixelFormat::Yuv420p,
            PixelFormat::Yuv422p,
            PixelFormat::Yuv444p,
            PixelFormat::Rgb24,
            PixelFormat::Rgba32,
            PixelFormat::Nv12,
        ] {
            let json = serde_json::to_string(&fmt).unwrap();
            let parsed: PixelFormat = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, fmt);
        }
    }

    #[test]
    fn media_info_multiple_audio_streams() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mkv,
            streams: vec![
                StreamInfo::Audio(AudioStreamInfo {
                    codec: AudioCodec::Aac,
                    sample_rate: 48000,
                    channels: 2,
                    sample_format: SampleFormat::F32,
                    bitrate: Some(128_000),
                    duration: None,
                }),
                StreamInfo::Audio(AudioStreamInfo {
                    codec: AudioCodec::Opus,
                    sample_rate: 48000,
                    channels: 6,
                    sample_format: SampleFormat::F32,
                    bitrate: Some(320_000),
                    duration: None,
                }),
                StreamInfo::Video(VideoStreamInfo {
                    codec: VideoCodec::H265,
                    width: 3840,
                    height: 2160,
                    pixel_format: PixelFormat::Yuv420p,
                    frame_rate: 60.0,
                    bitrate: None,
                    duration: None,
                }),
            ],
            duration: Some(Duration::from_secs(7200)),
            file_size: None,
            title: None,
            artist: None,
            album: None,
        };
        assert_eq!(info.audio_streams().len(), 2);
        assert_eq!(info.video_streams().len(), 1);
        assert_eq!(info.audio_streams()[1].channels, 6);
    }

    #[test]
    fn io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let tarang_err: TarangError = io_err.into();
        assert!(tarang_err.to_string().contains("file missing"));
    }
}
