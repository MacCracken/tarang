//! Roundtrip integration tests: mux → demux → verify
//!
//! These tests verify that data survives a full mux-then-demux cycle
//! for each supported container format.

use std::io::Cursor;
use std::time::Duration;
use tarang::core::{AudioCodec, ContainerFormat, TarangError};
use tarang::demux::{Demuxer, MuxConfig, Muxer, OggDemuxer, OggMuxer, WavDemuxer, WavMuxer};

// ---------------------------------------------------------------------------
// WAV roundtrip
// ---------------------------------------------------------------------------

#[test]
fn wav_mux_demux_roundtrip() {
    let config = MuxConfig {
        codec: AudioCodec::Pcm,
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
    };

    // Generate PCM data: 0.5 seconds of a 440 Hz sine wave
    let num_samples: usize = 22050;
    let pcm_data = generate_pcm_s16(44100, 2, num_samples);

    // Mux
    let mut buf = Cursor::new(Vec::new());
    {
        let mut mux = WavMuxer::new(&mut buf, config);
        mux.write_header().unwrap();
        mux.write_packet(&pcm_data).unwrap();
        mux.finalize().unwrap();
    }

    let wav_bytes = buf.into_inner();

    // Demux and verify metadata
    let mut demuxer = WavDemuxer::new(Cursor::new(wav_bytes.clone()));
    let info = demuxer.probe().unwrap();
    assert_eq!(info.format, ContainerFormat::Wav);
    assert!(info.has_audio());

    let audio_streams: Vec<_> = info.audio_streams().collect();
    assert_eq!(audio_streams.len(), 1);
    assert_eq!(audio_streams[0].sample_rate, 44100);
    assert_eq!(audio_streams[0].channels, 2);

    // Read all packets and verify total data size matches
    let mut recovered = Vec::new();
    loop {
        match demuxer.next_packet() {
            Ok(packet) => recovered.extend_from_slice(&packet.data),
            Err(TarangError::EndOfStream) | Err(TarangError::DemuxError(_)) => break,
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
    assert_eq!(
        recovered.len(),
        pcm_data.len(),
        "recovered PCM data length must match original"
    );
    assert_eq!(
        recovered, pcm_data,
        "recovered PCM data must be byte-identical to original"
    );
}

#[test]
fn wav_mux_demux_roundtrip_mono_48k() {
    let config = MuxConfig {
        codec: AudioCodec::Pcm,
        sample_rate: 48000,
        channels: 1,
        bits_per_sample: 16,
    };

    let num_samples: usize = 48000; // 1 second
    let pcm_data = generate_pcm_s16(48000, 1, num_samples);

    let mut buf = Cursor::new(Vec::new());
    {
        let mut mux = WavMuxer::new(&mut buf, config);
        mux.write_header().unwrap();
        mux.write_packet(&pcm_data).unwrap();
        mux.finalize().unwrap();
    }

    let wav_bytes = buf.into_inner();
    let mut demuxer = WavDemuxer::new(Cursor::new(wav_bytes));
    let info = demuxer.probe().unwrap();

    let audio_streams: Vec<_> = info.audio_streams().collect();
    assert_eq!(audio_streams[0].sample_rate, 48000);
    assert_eq!(audio_streams[0].channels, 1);

    // Verify duration is approximately 1 second
    let duration = info.duration.unwrap();
    assert!(
        (duration.as_secs_f64() - 1.0).abs() < 0.01,
        "duration should be ~1s, got {:.3}s",
        duration.as_secs_f64()
    );

    let mut recovered = Vec::new();
    loop {
        match demuxer.next_packet() {
            Ok(packet) => recovered.extend_from_slice(&packet.data),
            Err(TarangError::EndOfStream) | Err(TarangError::DemuxError(_)) => break,
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
    assert_eq!(recovered, pcm_data);
}

// ---------------------------------------------------------------------------
// WAV full decode pipeline roundtrip
// ---------------------------------------------------------------------------

#[test]
fn wav_mux_decode_pipeline_roundtrip() {
    let config = MuxConfig {
        codec: AudioCodec::Pcm,
        sample_rate: 44100,
        channels: 2,
        bits_per_sample: 16,
    };

    let pcm_data = generate_pcm_s16(44100, 2, 4410); // 0.1 seconds

    let mut buf = Cursor::new(Vec::new());
    {
        let mut mux = WavMuxer::new(&mut buf, config);
        mux.write_header().unwrap();
        mux.write_packet(&pcm_data).unwrap();
        mux.finalize().unwrap();
    }

    let wav_bytes = buf.into_inner();

    // Use the high-level FileDecoder
    let mut decoder =
        tarang::audio::FileDecoder::open(Box::new(Cursor::new(wav_bytes)), Some("wav")).unwrap();
    let decoded = decoder.decode_all().unwrap();

    assert_eq!(decoded.channels, 2);
    assert_eq!(decoded.sample_rate, 44100);
    assert!(decoded.num_samples > 0);
}

// ---------------------------------------------------------------------------
// OGG roundtrip (packet-level, since Opus/Vorbis packets are opaque)
// ---------------------------------------------------------------------------

#[test]
fn ogg_opus_mux_demux_roundtrip() {
    let config = MuxConfig {
        codec: AudioCodec::Opus,
        sample_rate: 48000,
        channels: 2,
        bits_per_sample: 16,
    };

    let packet_data = vec![0xABu8; 80];
    let num_packets = 5;

    let mut buf = Cursor::new(Vec::new());
    {
        let mut mux = OggMuxer::new(&mut buf, config).unwrap();
        mux.write_header().unwrap();
        for _ in 0..num_packets {
            mux.write_packet(&packet_data).unwrap();
        }
        mux.finalize().unwrap();
    }

    let ogg_bytes = buf.into_inner();
    let mut demuxer = OggDemuxer::new(Cursor::new(ogg_bytes));
    let info = demuxer.probe().unwrap();

    assert_eq!(info.format, ContainerFormat::Ogg);
    let audio_streams: Vec<_> = info.audio_streams().collect();
    assert_eq!(audio_streams.len(), 1);
    assert_eq!(audio_streams[0].codec, AudioCodec::Opus);
    assert_eq!(audio_streams[0].sample_rate, 48000);
    assert_eq!(audio_streams[0].channels, 2);

    // Read packets back and verify data
    let mut count = 0;
    loop {
        match demuxer.next_packet() {
            Ok(p) => {
                assert_eq!(p.data.len(), packet_data.len());
                assert_eq!(p.data.as_ref(), packet_data.as_slice());
                count += 1;
            }
            Err(TarangError::EndOfStream) | Err(TarangError::DemuxError(_)) => break,
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
    assert!(count >= 1, "should recover at least one data packet");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate 16-bit signed PCM data (little-endian) with a 440 Hz sine wave.
fn generate_pcm_s16(sample_rate: u32, channels: u16, num_samples: usize) -> Vec<u8> {
    let total_frames = num_samples * channels as usize;
    let mut data = Vec::with_capacity(total_frames * 2);
    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let sample = ((t * 440.0 * std::f32::consts::TAU).sin() * 16000.0) as i16;
        for _ in 0..channels {
            data.extend_from_slice(&sample.to_le_bytes());
        }
    }
    data
}
