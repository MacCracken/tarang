use bytes::Bytes;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::time::Duration;
use tarang::ai::{self, SceneDetectionConfig, SceneDetector, luminance_variance};
use tarang::core::{
    AudioBuffer, AudioCodec, AudioStreamInfo, ContainerFormat, MediaInfo, PixelFormat,
    SampleFormat, StreamInfo, VideoCodec, VideoFrame, VideoStreamInfo,
};

fn make_sine_f32(sample_rate: u32, num_samples: usize) -> AudioBuffer {
    let mut data = Vec::with_capacity(num_samples * 4);
    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let s = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
        data.extend_from_slice(&s.to_le_bytes());
    }
    AudioBuffer {
        data: Bytes::from(data),
        sample_format: SampleFormat::F32,
        channels: 1,
        sample_rate,
        num_samples,
        timestamp: Duration::ZERO,
    }
}

fn make_yuv_frame(width: u32, height: u32, y_value: u8, ts_ms: u64) -> VideoFrame {
    let y_size = (width * height) as usize;
    let chroma_w = width.div_ceil(2) as usize;
    let chroma_h = height.div_ceil(2) as usize;
    let chroma_size = chroma_w * chroma_h;
    let mut data = vec![y_value; y_size];
    data.extend(vec![128u8; chroma_size * 2]);
    VideoFrame {
        data: Bytes::from(data),
        width,
        height,
        pixel_format: PixelFormat::Yuv420p,
        timestamp: Duration::from_millis(ts_ms),
    }
}

fn bench_fingerprint(c: &mut Criterion) {
    let buf = make_sine_f32(44100, 44100);
    c.bench_function("fingerprint_1s_mono_44100", |b| {
        b.iter(|| ai::compute_fingerprint(black_box(&buf), &Default::default()))
    });
}

fn bench_fingerprint_long(c: &mut Criterion) {
    let buf = make_sine_f32(44100, 44100 * 10);
    c.bench_function("fingerprint_10s_mono_44100", |b| {
        b.iter(|| ai::compute_fingerprint(black_box(&buf), &Default::default()))
    });
}

fn bench_scene_detection(c: &mut Criterion) {
    let frames: Vec<VideoFrame> = (0..60)
        .map(|i| {
            let y = if i < 30 { 50 } else { 200 };
            make_yuv_frame(320, 240, y, i * 33)
        })
        .collect();

    c.bench_function("scene_detect_60_frames_320x240", |b| {
        b.iter(|| {
            let mut detector = SceneDetector::new(SceneDetectionConfig::default());
            for frame in &frames {
                detector.feed_frame(black_box(frame));
            }
            detector.finish().len()
        })
    });
}

fn bench_scene_detection_hd(c: &mut Criterion) {
    let frames: Vec<VideoFrame> = (0..30)
        .map(|i| make_yuv_frame(1920, 1080, (i * 8) as u8, i * 33))
        .collect();

    c.bench_function("scene_detect_30_frames_1080p", |b| {
        b.iter(|| {
            let mut detector = SceneDetector::new(SceneDetectionConfig::default());
            for frame in &frames {
                detector.feed_frame(black_box(frame));
            }
            detector.finish().len()
        })
    });
}

fn bench_luminance_variance(c: &mut Criterion) {
    let frame = make_yuv_frame(1920, 1080, 128, 0);
    c.bench_function("luminance_variance_1080p", |b| {
        b.iter(|| luminance_variance(black_box(&frame)))
    });
}

fn bench_analyze_media(c: &mut Criterion) {
    let info = MediaInfo {
        id: uuid::Uuid::new_v4(),
        format: ContainerFormat::Mp4,
        streams: vec![
            StreamInfo::Audio(AudioStreamInfo {
                codec: AudioCodec::Aac,
                sample_rate: 44100,
                channels: 2,
                sample_format: SampleFormat::F32,
                bitrate: Some(128000),
                duration: Some(Duration::from_secs(120)),
            }),
            StreamInfo::Video(VideoStreamInfo {
                codec: VideoCodec::H264,
                width: 1920,
                height: 1080,
                pixel_format: PixelFormat::Yuv420p,
                frame_rate: 30.0,
                bitrate: Some(5_000_000),
                duration: Some(Duration::from_secs(120)),
            }),
        ],
        duration: Some(Duration::from_secs(120)),
        file_size: Some(50_000_000),
        title: Some("Test".to_string()),
        artist: None,
        album: None,
        metadata: std::collections::HashMap::new(),
    };

    c.bench_function("analyze_media_2min_h264_aac", |b| {
        b.iter(|| ai::analyze_media(black_box(&info)))
    });
}

criterion_group!(
    benches,
    bench_fingerprint,
    bench_fingerprint_long,
    bench_scene_detection,
    bench_scene_detection_hd,
    bench_luminance_variance,
    bench_analyze_media,
);
criterion_main!(benches);
