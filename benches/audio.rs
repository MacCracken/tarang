use bytes::Bytes;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::time::Duration;
use tarang::audio::{self, AudioEncoder, EncoderConfig};
use tarang::core::{AudioBuffer, AudioCodec, SampleFormat};

fn make_sine_f32(sample_rate: u32, channels: u16, num_frames: usize) -> AudioBuffer {
    let total = num_frames * channels as usize;
    let mut data = Vec::with_capacity(total * 4);
    for i in 0..total {
        let t = i as f32 / sample_rate as f32;
        let s = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
        data.extend_from_slice(&s.to_le_bytes());
    }
    AudioBuffer {
        data: Bytes::from(data),
        sample_format: SampleFormat::F32,
        channels,
        sample_rate,
        num_frames,
        timestamp: Duration::ZERO,
    }
}

fn bench_resample_linear(c: &mut Criterion) {
    let buf = make_sine_f32(44100, 2, 44100);
    c.bench_function("resample_linear_44100_to_48000", |b| {
        b.iter(|| audio::resample(black_box(&buf), 48000).unwrap())
    });
}

fn bench_resample_sinc(c: &mut Criterion) {
    let buf = make_sine_f32(44100, 2, 44100);
    c.bench_function("resample_sinc_44100_to_48000", |b| {
        b.iter(|| audio::resample_sinc(black_box(&buf), 48000, 32).unwrap())
    });
}

fn bench_resample_downsample(c: &mut Criterion) {
    let buf = make_sine_f32(48000, 1, 48000);
    c.bench_function("resample_linear_48000_to_16000_mono", |b| {
        b.iter(|| audio::resample(black_box(&buf), 16000).unwrap())
    });
}

fn bench_mix_stereo_to_mono(c: &mut Criterion) {
    let buf = make_sine_f32(44100, 2, 44100);
    c.bench_function("mix_stereo_to_mono_1s", |b| {
        b.iter(|| audio::mix_channels(black_box(&buf), audio::ChannelLayout::Mono).unwrap())
    });
}

fn bench_flac_encode(c: &mut Criterion) {
    let buf = make_sine_f32(44100, 2, 44100);
    let config = EncoderConfig {
        codec: AudioCodec::Flac,
        channels: 2,
        sample_rate: 44100,
        bits_per_sample: 16,
    };
    c.bench_function("flac_encode_1s_stereo", |b| {
        b.iter(|| {
            let mut enc = audio::FlacEncoder::new(&config).unwrap();
            enc.encode(black_box(&buf)).unwrap()
        })
    });
}

fn bench_pcm_encode(c: &mut Criterion) {
    let buf = make_sine_f32(44100, 2, 44100);
    let config = EncoderConfig {
        codec: AudioCodec::Pcm,
        channels: 2,
        sample_rate: 44100,
        bits_per_sample: 16,
    };
    c.bench_function("pcm_encode_1s_stereo_16bit", |b| {
        b.iter(|| {
            let mut enc = audio::PcmEncoder::new(&config).unwrap();
            enc.encode(black_box(&buf)).unwrap()
        })
    });
}

criterion_group!(
    benches,
    bench_resample_linear,
    bench_resample_sinc,
    bench_resample_downsample,
    bench_mix_stereo_to_mono,
    bench_flac_encode,
    bench_pcm_encode,
);
criterion_main!(benches);
