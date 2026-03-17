//! tarang-ai — AI features for the Tarang media framework
//!
//! Media analysis, content classification, and transcription routing via hoosh.
//! Connects to the AGNOS LLM gateway for AI-powered media understanding.

pub mod daimon;
pub mod fingerprint;
pub mod scene;
pub mod thumbnail;
pub mod transcribe;

pub use daimon::{
    ContentDescription, DaimonClient, DaimonConfig, HooshLlmClient, HooshLlmConfig, RagResult,
    SimilarMedia,
};
pub use fingerprint::{
    AudioFingerprint, FingerprintConfig, compute_fingerprint, fingerprint_match,
};
pub use scene::{
    SceneBoundary, SceneBoundaryType, SceneDetectionConfig, SceneDetector, detect_scenes,
};
pub use thumbnail::{
    Thumbnail, ThumbnailConfig, ThumbnailFormat, ThumbnailGenerator, generate_thumbnails,
    luminance_variance, yuv420p_to_rgb24,
};
pub use transcribe::{
    HooshClient, HooshConfig, WhisperModel, encode_wav_bytes, prepare_audio_for_transcription,
};

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tarang_core::{AudioCodec, MediaInfo, VideoCodec};

/// Media content classification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentType {
    Music,
    Speech,
    Podcast,
    Movie,
    Clip,
    Screencast,
    Animation,
    Unknown,
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Music => write!(f, "music"),
            Self::Speech => write!(f, "speech"),
            Self::Podcast => write!(f, "podcast"),
            Self::Movie => write!(f, "movie"),
            Self::Clip => write!(f, "clip"),
            Self::Screencast => write!(f, "screencast"),
            Self::Animation => write!(f, "animation"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Media analysis result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAnalysis {
    pub content_type: ContentType,
    pub quality_score: f32,
    pub codec_recommendation: Option<String>,
    pub estimated_complexity: f32,
    pub tags: Vec<String>,
}

/// Transcription request to route through hoosh
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionRequest {
    pub audio_codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_secs: f64,
    pub language_hint: Option<String>,
}

/// Transcription result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    pub language: String,
    pub confidence: f32,
    pub segments: Vec<TranscriptionSegment>,
}

/// A timed segment of transcription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub confidence: f32,
}

/// Analyze media content and classify it
pub fn analyze_media(info: &MediaInfo) -> MediaAnalysis {
    let has_video = info.has_video();
    let has_audio = info.has_audio();
    let duration = info.duration.unwrap_or(Duration::ZERO);

    // Classification thresholds:
    // - Audio-only: ≤10min → Music, >10min → Podcast
    // - Audio+Video: ≤1hr → Clip, >1hr → Movie
    const PODCAST_THRESHOLD: Duration = Duration::from_secs(600);
    const MOVIE_THRESHOLD: Duration = Duration::from_secs(3600);

    let content_type = match (has_video, has_audio) {
        (false, true) => {
            if duration > PODCAST_THRESHOLD {
                ContentType::Podcast
            } else {
                ContentType::Music
            }
        }
        (true, true) => {
            if duration > MOVIE_THRESHOLD {
                ContentType::Movie
            } else {
                ContentType::Clip
            }
        }
        (true, false) => ContentType::Animation,
        (false, false) => ContentType::Unknown,
    };

    let quality_score = compute_quality_score(info);
    let codec_recommendation = suggest_codec(info);

    let video_streams = info.video_streams();
    let estimated_complexity = if let Some(v) = video_streams.first() {
        let pixels = v.width as f64 * v.height as f64;
        let fps = v.frame_rate;
        (pixels * fps / 1_000_000.0).min(100.0) as f32
    } else {
        0.0
    };

    let mut tags = Vec::new();
    if has_video {
        tags.push("video".to_string());
    }
    if has_audio {
        tags.push("audio".to_string());
    }
    for v in info.video_streams() {
        if v.width >= 3840 {
            tags.push("4k".to_string());
        } else if v.width >= 1920 {
            tags.push("hd".to_string());
        }
    }

    MediaAnalysis {
        content_type,
        quality_score,
        codec_recommendation,
        estimated_complexity,
        tags,
    }
}

fn compute_quality_score(info: &MediaInfo) -> f32 {
    let mut score: f32 = 50.0;

    for v in info.video_streams() {
        if v.width >= 1920 {
            score += 20.0;
        }
        if v.frame_rate >= 60.0 {
            score += 10.0;
        }
        if matches!(v.codec, VideoCodec::Av1 | VideoCodec::H265) {
            score += 10.0;
        }
    }

    for a in info.audio_streams() {
        if a.sample_rate >= 48000 {
            score += 5.0;
        }
        if matches!(a.codec, AudioCodec::Flac | AudioCodec::Alac) {
            score += 5.0;
        }
    }

    score.min(100.0)
}

fn suggest_codec(info: &MediaInfo) -> Option<String> {
    for v in info.video_streams() {
        match v.codec {
            VideoCodec::H264 if v.width >= 1920 => {
                return Some("Consider AV1 for better compression at this resolution".to_string());
            }
            VideoCodec::Vp8 => {
                return Some("VP9 offers significantly better quality at same bitrate".to_string());
            }
            _ => {}
        }
    }
    None
}

/// Create a transcription request from media info
pub fn prepare_transcription(
    info: &MediaInfo,
    language_hint: Option<String>,
) -> Option<TranscriptionRequest> {
    let audio = info.audio_streams().into_iter().next()?;
    let duration = info.duration.unwrap_or(Duration::ZERO);

    Some(TranscriptionRequest {
        audio_codec: audio.codec.to_string(),
        sample_rate: audio.sample_rate,
        channels: audio.channels,
        duration_secs: duration.as_secs_f64(),
        language_hint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tarang_core::*;
    use uuid::Uuid;

    fn make_video_info(
        width: u32,
        height: u32,
        codec: VideoCodec,
        duration_secs: u64,
    ) -> MediaInfo {
        MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mp4,
            streams: vec![
                StreamInfo::Video(VideoStreamInfo {
                    codec,
                    width,
                    height,
                    pixel_format: PixelFormat::Yuv420p,
                    frame_rate: 30.0,
                    bitrate: Some(5_000_000),
                    duration: Some(Duration::from_secs(duration_secs)),
                }),
                StreamInfo::Audio(AudioStreamInfo {
                    codec: AudioCodec::Aac,
                    sample_rate: 48000,
                    channels: 2,
                    sample_format: SampleFormat::F32,
                    bitrate: Some(128_000),
                    duration: Some(Duration::from_secs(duration_secs)),
                }),
            ],
            duration: Some(Duration::from_secs(duration_secs)),
            file_size: None,
            title: None,
            artist: None,
            album: None,
        }
    }

    fn make_audio_info(codec: AudioCodec, duration_secs: u64) -> MediaInfo {
        MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Flac,
            streams: vec![StreamInfo::Audio(AudioStreamInfo {
                codec,
                sample_rate: 44100,
                channels: 2,
                sample_format: SampleFormat::I16,
                bitrate: None,
                duration: Some(Duration::from_secs(duration_secs)),
            })],
            duration: Some(Duration::from_secs(duration_secs)),
            file_size: None,
            title: None,
            artist: None,
            album: None,
        }
    }

    #[test]
    fn classify_music() {
        let info = make_audio_info(AudioCodec::Flac, 240);
        let analysis = analyze_media(&info);
        assert_eq!(analysis.content_type, ContentType::Music);
    }

    #[test]
    fn classify_podcast() {
        let info = make_audio_info(AudioCodec::Mp3, 3600);
        let analysis = analyze_media(&info);
        assert_eq!(analysis.content_type, ContentType::Podcast);
    }

    #[test]
    fn classify_clip() {
        let info = make_video_info(1920, 1080, VideoCodec::H264, 60);
        let analysis = analyze_media(&info);
        assert_eq!(analysis.content_type, ContentType::Clip);
    }

    #[test]
    fn classify_movie() {
        let info = make_video_info(1920, 1080, VideoCodec::H264, 7200);
        let analysis = analyze_media(&info);
        assert_eq!(analysis.content_type, ContentType::Movie);
    }

    #[test]
    fn quality_score_4k() {
        let info = make_video_info(3840, 2160, VideoCodec::Av1, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.quality_score > 80.0);
    }

    #[test]
    fn quality_score_sd() {
        let info = make_video_info(640, 480, VideoCodec::H264, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.quality_score < 80.0);
    }

    #[test]
    fn quality_score_lossless_audio() {
        let info = make_audio_info(AudioCodec::Flac, 240);
        let analysis = analyze_media(&info);
        assert!(analysis.quality_score >= 55.0); // base + lossless bonus
    }

    #[test]
    fn suggest_av1_for_hd_h264() {
        let info = make_video_info(1920, 1080, VideoCodec::H264, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.codec_recommendation.is_some());
        assert!(analysis.codec_recommendation.unwrap().contains("AV1"));
    }

    #[test]
    fn no_suggestion_for_av1() {
        let info = make_video_info(1920, 1080, VideoCodec::Av1, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.codec_recommendation.is_none());
    }

    #[test]
    fn tags_include_hd() {
        let info = make_video_info(1920, 1080, VideoCodec::H264, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.tags.contains(&"hd".to_string()));
        assert!(analysis.tags.contains(&"video".to_string()));
        assert!(analysis.tags.contains(&"audio".to_string()));
    }

    #[test]
    fn tags_include_4k() {
        let info = make_video_info(3840, 2160, VideoCodec::Av1, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.tags.contains(&"4k".to_string()));
    }

    #[test]
    fn complexity_estimate() {
        let info = make_video_info(1920, 1080, VideoCodec::H264, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.estimated_complexity > 0.0);
    }

    #[test]
    fn prepare_transcription_from_video() {
        let info = make_video_info(1920, 1080, VideoCodec::H264, 120);
        let req = prepare_transcription(&info, Some("en".to_string()));
        assert!(req.is_some());
        let req = req.unwrap();
        assert_eq!(req.audio_codec, "AAC");
        assert_eq!(req.sample_rate, 48000);
        assert_eq!(req.channels, 2);
        assert_eq!(req.language_hint, Some("en".to_string()));
    }

    #[test]
    fn prepare_transcription_audio_only() {
        let info = make_audio_info(AudioCodec::Mp3, 300);
        let req = prepare_transcription(&info, None);
        assert!(req.is_some());
        assert_eq!(req.unwrap().audio_codec, "MP3");
    }

    #[test]
    fn prepare_transcription_no_audio() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mp4,
            streams: vec![StreamInfo::Video(VideoStreamInfo {
                codec: VideoCodec::H264,
                width: 1920,
                height: 1080,
                pixel_format: PixelFormat::Yuv420p,
                frame_rate: 30.0,
                bitrate: None,
                duration: None,
            })],
            duration: None,
            file_size: None,
            title: None,
            artist: None,
            album: None,
        };
        assert!(prepare_transcription(&info, None).is_none());
    }

    #[test]
    fn content_type_display() {
        assert_eq!(ContentType::Music.to_string(), "music");
        assert_eq!(ContentType::Movie.to_string(), "movie");
        assert_eq!(ContentType::Podcast.to_string(), "podcast");
        assert_eq!(ContentType::Unknown.to_string(), "unknown");
    }

    #[test]
    fn content_type_serialization() {
        let json = serde_json::to_string(&ContentType::Music).unwrap();
        let parsed: ContentType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ContentType::Music);
    }

    #[test]
    fn content_type_display_all() {
        assert_eq!(ContentType::Speech.to_string(), "speech");
        assert_eq!(ContentType::Clip.to_string(), "clip");
        assert_eq!(ContentType::Screencast.to_string(), "screencast");
        assert_eq!(ContentType::Animation.to_string(), "animation");
    }

    #[test]
    fn content_type_serialization_all() {
        for ct in [
            ContentType::Music,
            ContentType::Speech,
            ContentType::Podcast,
            ContentType::Movie,
            ContentType::Clip,
            ContentType::Screencast,
            ContentType::Animation,
            ContentType::Unknown,
        ] {
            let json = serde_json::to_string(&ct).unwrap();
            let parsed: ContentType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, ct);
        }
    }

    #[test]
    fn classify_boundary_music_podcast_600s() {
        // Exactly 600s should be Music (not >600)
        let info = make_audio_info(AudioCodec::Mp3, 600);
        let analysis = analyze_media(&info);
        assert_eq!(analysis.content_type, ContentType::Music);
    }

    #[test]
    fn classify_boundary_podcast_601s() {
        let info = make_audio_info(AudioCodec::Mp3, 601);
        let analysis = analyze_media(&info);
        assert_eq!(analysis.content_type, ContentType::Podcast);
    }

    #[test]
    fn classify_boundary_clip_movie_3600s() {
        // Exactly 3600s should be Clip (not >3600)
        let info = make_video_info(1920, 1080, VideoCodec::H264, 3600);
        let analysis = analyze_media(&info);
        assert_eq!(analysis.content_type, ContentType::Clip);
    }

    #[test]
    fn classify_boundary_movie_3601s() {
        let info = make_video_info(1920, 1080, VideoCodec::H264, 3601);
        let analysis = analyze_media(&info);
        assert_eq!(analysis.content_type, ContentType::Movie);
    }

    #[test]
    fn classify_animation_video_only() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mp4,
            streams: vec![StreamInfo::Video(VideoStreamInfo {
                codec: VideoCodec::H264,
                width: 1920,
                height: 1080,
                pixel_format: PixelFormat::Yuv420p,
                frame_rate: 24.0,
                bitrate: None,
                duration: None,
            })],
            duration: Some(Duration::from_secs(60)),
            file_size: None,
            title: None,
            artist: None,
            album: None,
        };
        let analysis = analyze_media(&info);
        assert_eq!(analysis.content_type, ContentType::Animation);
        assert!(analysis.tags.contains(&"video".to_string()));
        assert!(!analysis.tags.contains(&"audio".to_string()));
    }

    #[test]
    fn classify_unknown_no_streams() {
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
        let analysis = analyze_media(&info);
        assert_eq!(analysis.content_type, ContentType::Unknown);
        assert!(analysis.tags.is_empty());
        assert_eq!(analysis.estimated_complexity, 0.0);
    }

    #[test]
    fn suggest_vp9_for_vp8() {
        let info = make_video_info(640, 480, VideoCodec::Vp8, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.codec_recommendation.is_some());
        assert!(analysis.codec_recommendation.unwrap().contains("VP9"));
    }

    #[test]
    fn no_suggestion_for_h265() {
        let info = make_video_info(3840, 2160, VideoCodec::H265, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.codec_recommendation.is_none());
    }

    #[test]
    fn quality_score_high_sample_rate_audio() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Flac,
            streams: vec![StreamInfo::Audio(AudioStreamInfo {
                codec: AudioCodec::Flac,
                sample_rate: 96000,
                channels: 2,
                sample_format: SampleFormat::I32,
                bitrate: None,
                duration: None,
            })],
            duration: Some(Duration::from_secs(60)),
            file_size: None,
            title: None,
            artist: None,
            album: None,
        };
        let analysis = analyze_media(&info);
        // base 50 + sample_rate>=48k bonus 5 + lossless bonus 5 = 60
        assert!(analysis.quality_score >= 60.0);
    }

    #[test]
    fn quality_score_capped_at_100() {
        // 4K H265 60fps + lossless high-rate audio should cap at 100
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mkv,
            streams: vec![
                StreamInfo::Video(VideoStreamInfo {
                    codec: VideoCodec::H265,
                    width: 3840,
                    height: 2160,
                    pixel_format: PixelFormat::Yuv420p,
                    frame_rate: 120.0,
                    bitrate: None,
                    duration: None,
                }),
                StreamInfo::Audio(AudioStreamInfo {
                    codec: AudioCodec::Flac,
                    sample_rate: 96000,
                    channels: 2,
                    sample_format: SampleFormat::I32,
                    bitrate: None,
                    duration: None,
                }),
            ],
            duration: Some(Duration::from_secs(60)),
            file_size: None,
            title: None,
            artist: None,
            album: None,
        };
        let analysis = analyze_media(&info);
        assert!(analysis.quality_score <= 100.0);
    }

    #[test]
    fn estimated_complexity_capped() {
        // Very high resolution + fps should cap at 100
        let info = make_video_info(7680, 4320, VideoCodec::Av1, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.estimated_complexity <= 100.0);
    }

    #[test]
    fn no_suggestion_for_sd_h264() {
        // H264 below 1920 should not trigger AV1 suggestion
        let info = make_video_info(1280, 720, VideoCodec::H264, 60);
        let analysis = analyze_media(&info);
        assert!(analysis.codec_recommendation.is_none());
    }

    #[test]
    fn prepare_transcription_no_duration() {
        let info = MediaInfo {
            id: Uuid::new_v4(),
            format: ContainerFormat::Mp4,
            streams: vec![StreamInfo::Audio(AudioStreamInfo {
                codec: AudioCodec::Opus,
                sample_rate: 48000,
                channels: 1,
                sample_format: SampleFormat::F32,
                bitrate: None,
                duration: None,
            })],
            duration: None,
            file_size: None,
            title: None,
            artist: None,
            album: None,
        };
        let req = prepare_transcription(&info, None);
        assert!(req.is_some());
        assert_eq!(req.unwrap().duration_secs, 0.0);
    }

    #[test]
    fn media_analysis_serialization() {
        let info = make_video_info(1920, 1080, VideoCodec::H264, 60);
        let analysis = analyze_media(&info);
        let json = serde_json::to_string(&analysis).unwrap();
        let parsed: MediaAnalysis = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content_type, analysis.content_type);
        assert_eq!(parsed.tags, analysis.tags);
    }
}
