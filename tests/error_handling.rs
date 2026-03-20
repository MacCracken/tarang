//! Integration tests for error handling across module boundaries.
//!
//! These tests verify that tarang gracefully handles malformed input,
//! corrupt data, and edge cases at the public API level.

use bytes::Bytes;
use std::io::Cursor;
use std::time::Duration;
use tarang::core::{AudioBuffer, PixelFormat, SampleFormat, VideoFrame};
use tarang::demux::{Demuxer, Mp4Demuxer, OggDemuxer, WavDemuxer};

// ---------------------------------------------------------------------------
// Demuxer error paths
// ---------------------------------------------------------------------------

#[test]
fn empty_file_all_demuxers() {
    let empty = Cursor::new(Vec::<u8>::new());
    let mut wav = WavDemuxer::new(empty.clone());
    assert!(wav.probe().is_err());

    let mut mp4 = Mp4Demuxer::new(empty.clone());
    assert!(mp4.probe().is_err());

    let mut ogg = OggDemuxer::new(empty);
    assert!(ogg.probe().is_err());
}

#[test]
fn random_garbage_all_demuxers() {
    let garbage = Cursor::new(vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x11, 0x22, 0x33]);

    let mut wav = WavDemuxer::new(garbage.clone());
    assert!(wav.probe().is_err());

    let mut mp4 = Mp4Demuxer::new(garbage.clone());
    assert!(mp4.probe().is_err());

    let mut ogg = OggDemuxer::new(garbage);
    assert!(ogg.probe().is_err());
}

#[test]
fn wav_truncated_header() {
    // RIFF header but no fmt chunk
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&100u32.to_le_bytes());
    data.extend_from_slice(b"WAVE");
    // No fmt chunk follows

    let mut demuxer = WavDemuxer::new(Cursor::new(data));
    assert!(demuxer.probe().is_err());
}

#[test]
fn mp4_ftyp_only_no_moov() {
    let mut data = Vec::new();
    // ftyp box
    let size = 20u32;
    data.extend_from_slice(&size.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    data.extend_from_slice(b"isom");
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(b"isom");

    let mut demuxer = Mp4Demuxer::new(Cursor::new(data));
    assert!(demuxer.probe().is_err());
}

#[test]
fn ogg_valid_header_no_data_pages() {
    // Single BOS page with no following data pages
    let mut page = Vec::new();
    page.extend_from_slice(b"OggS"); // capture pattern
    page.push(0); // version
    page.push(0x02); // BOS flag
    page.extend_from_slice(&0i64.to_le_bytes()); // granule
    page.extend_from_slice(&1u32.to_le_bytes()); // serial
    page.extend_from_slice(&0u32.to_le_bytes()); // sequence
    page.extend_from_slice(&0u32.to_le_bytes()); // CRC placeholder
    page.push(1); // 1 segment
    page.push(30); // segment size = 30 bytes

    // Vorbis identification header (minimal)
    let mut body = vec![0x01]; // packet type
    body.extend_from_slice(b"vorbis");
    body.extend_from_slice(&0u32.to_le_bytes()); // version
    body.push(2); // channels
    body.extend_from_slice(&44100u32.to_le_bytes()); // sample rate
    body.extend_from_slice(&0u32.to_le_bytes()); // bitrate max
    body.extend_from_slice(&0u32.to_le_bytes()); // bitrate nominal
    // Truncate to 30 bytes
    body.resize(30, 0);

    page.extend_from_slice(&body);

    // Fix CRC
    let crc = ogg_crc32(&page);
    page[22..26].copy_from_slice(&crc.to_le_bytes());

    let mut demuxer = OggDemuxer::new(Cursor::new(page));
    // Probe may succeed (has BOS), but next_packet should fail or return EndOfStream
    let _ = demuxer.probe();
    // Either probe failed or next_packet gives EndOfStream/error
    match demuxer.next_packet() {
        Err(_) => {} // expected
        Ok(p) => {
            // If we got a packet, the next one should be EndOfStream
            assert!(matches!(
                demuxer.next_packet(),
                Err(tarang::core::TarangError::EndOfStream)
                    | Err(tarang::core::TarangError::DemuxError(_))
            ));
            let _ = p;
        }
    }
}

fn ogg_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for &byte in data {
        crc = (crc << 8) ^ OGG_CRC_TABLE[((crc >> 24) as u8 ^ byte) as usize];
    }
    crc
}

const OGG_CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = (i as u32) << 24;
        let mut j = 0;
        while j < 8 {
            if r & 0x80000000 != 0 {
                r = (r << 1) ^ 0x04C11DB7;
            } else {
                r <<= 1;
            }
            j += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
};

#[test]
fn next_packet_before_probe() {
    let wav_data = make_valid_wav(44100, 1, 100);
    let mut demuxer = WavDemuxer::new(Cursor::new(wav_data));
    // Reading packets without probing first
    let result = demuxer.next_packet();
    // Should either error or return EndOfStream
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Audio processing error paths
// ---------------------------------------------------------------------------

#[test]
fn resample_zero_target_rate() {
    let buf = make_audio_buffer(44100, 1, 1000);
    let result = tarang::audio::resample(&buf, 0);
    assert!(result.is_err());
}

#[test]
fn resample_same_rate_passthrough() {
    let buf = make_audio_buffer(44100, 2, 1000);
    let result = tarang::audio::resample(&buf, 44100).unwrap();
    assert_eq!(result.sample_rate, 44100);
    assert_eq!(result.num_frames, 1000);
    // Data should be identical (Bytes clone is zero-copy)
    assert_eq!(result.data, buf.data);
}

#[test]
fn resample_empty_buffer() {
    let buf = AudioBuffer {
        data: Bytes::new(),
        sample_format: SampleFormat::F32,
        channels: 1,
        sample_rate: 44100,
        num_frames: 0,
        timestamp: Duration::ZERO,
    };
    let result = tarang::audio::resample(&buf, 48000);
    assert!(result.is_err());
}

#[test]
fn mix_zero_channels() {
    let buf = AudioBuffer {
        data: Bytes::from(vec![0u8; 400]),
        sample_format: SampleFormat::F32,
        channels: 0,
        sample_rate: 44100,
        num_frames: 100,
        timestamp: Duration::ZERO,
    };
    let result = tarang::audio::mix_channels(&buf, tarang::audio::ChannelLayout::Mono);
    assert!(result.is_err());
}

#[test]
fn mix_mono_to_mono_passthrough() {
    let buf = make_audio_buffer(44100, 1, 1000);
    let result = tarang::audio::mix_channels(&buf, tarang::audio::ChannelLayout::Mono).unwrap();
    assert_eq!(result.channels, 1);
    assert_eq!(result.num_frames, 1000);
}

// ---------------------------------------------------------------------------
// AI module error paths
// ---------------------------------------------------------------------------

#[test]
fn fingerprint_empty_buffer() {
    let buf = AudioBuffer {
        data: Bytes::new(),
        sample_format: SampleFormat::F32,
        channels: 1,
        sample_rate: 44100,
        num_frames: 0,
        timestamp: Duration::ZERO,
    };
    let fp = tarang::ai::compute_fingerprint(&buf, &Default::default()).unwrap();
    assert!(fp.hashes.is_empty());
}

#[test]
fn scene_detector_zero_dimension_frames() {
    let mut detector = tarang::ai::SceneDetector::new(Default::default());

    let frame = VideoFrame {
        data: Bytes::new(),
        width: 0,
        height: 0,
        pixel_format: PixelFormat::Yuv420p,
        timestamp: Duration::ZERO,
    };

    // Should not panic, should return None
    assert!(detector.feed_frame(&frame).is_none());
}

#[test]
fn luminance_variance_empty_frame() {
    let frame = VideoFrame {
        data: Bytes::new(),
        width: 0,
        height: 0,
        pixel_format: PixelFormat::Yuv420p,
        timestamp: Duration::ZERO,
    };
    let var = tarang::ai::luminance_variance(&frame);
    // Should not panic; variance of empty frame is 0
    assert!(var >= 0.0);
}

#[test]
fn analyze_media_minimal_info() {
    use tarang::core::{ContainerFormat, MediaInfo};

    let info = MediaInfo {
        id: uuid::Uuid::new_v4(),
        format: ContainerFormat::Wav,
        streams: vec![],
        duration: None,
        file_size: None,
        title: None,
        artist: None,
        album: None,
        metadata: std::collections::HashMap::new(),
    };

    // Should not panic even with no streams
    let analysis = tarang::ai::analyze_media(&info);
    assert!(analysis.codec_recommendation.is_some() || analysis.codec_recommendation.is_none());
}

// ---------------------------------------------------------------------------
// Cross-module integration: demux → decode pipeline
// ---------------------------------------------------------------------------

#[test]
fn wav_demux_to_decode_pipeline() {
    let wav = make_valid_wav(44100, 2, 4410); // 0.1 second stereo

    // Demux
    let mut demuxer = WavDemuxer::new(Cursor::new(wav.clone()));
    let info = demuxer.probe().unwrap();
    assert!(info.duration.is_some());

    // Read all packets
    let mut total_bytes = 0;
    while let Ok(packet) = demuxer.next_packet() {
        total_bytes += packet.data.len();
    }
    // 4410 samples * 2 channels * 2 bytes (16-bit) = 17640
    assert_eq!(total_bytes, 4410 * 2 * 2);

    // Full decode pipeline
    let mut decoder =
        tarang::audio::FileDecoder::open(Box::new(Cursor::new(wav)), Some("wav")).unwrap();
    let decoded = decoder.decode_all().unwrap();
    assert_eq!(decoded.channels, 2);
    assert_eq!(decoded.sample_rate, 44100);
    assert!(decoded.num_frames > 0);
}

#[test]
fn decode_resample_mix_pipeline() {
    let wav = make_valid_wav(48000, 2, 4800); // 0.1 second stereo 48kHz

    let mut decoder =
        tarang::audio::FileDecoder::open(Box::new(Cursor::new(wav)), Some("wav")).unwrap();
    let decoded = decoder.decode_all().unwrap();

    // Resample 48k → 16k
    let resampled = tarang::audio::resample(&decoded, 16000).unwrap();
    assert_eq!(resampled.sample_rate, 16000);
    assert!(resampled.num_frames < decoded.num_frames);

    // Mix stereo → mono
    let mono = tarang::audio::mix_channels(&resampled, tarang::audio::ChannelLayout::Mono).unwrap();
    assert_eq!(mono.channels, 1);
    assert_eq!(mono.sample_rate, 16000);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_audio_buffer(sample_rate: u32, channels: u16, num_frames: usize) -> AudioBuffer {
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

fn make_valid_wav(sample_rate: u32, channels: u16, num_samples: usize) -> Vec<u8> {
    let bits_per_sample: u16 = 16;
    let bytes_per_sample = bits_per_sample / 8;
    let block_align = channels * bytes_per_sample;
    let byte_rate = sample_rate * block_align as u32;
    let data_size = (num_samples * channels as usize * bytes_per_sample as usize) as u32;
    let file_size = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());

    // Generate sine wave PCM16 data
    for i in 0..(num_samples * channels as usize) {
        let t = i as f32 / sample_rate as f32;
        let sample = ((t * 440.0 * std::f32::consts::TAU).sin() * 16000.0) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
    }

    wav
}
