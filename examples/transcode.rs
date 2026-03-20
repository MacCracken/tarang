//! Transcode an audio file: decode → resample → encode → mux.
//!
//! Usage: cargo run --example transcode -- input.mp3 output.wav
//!
//! Decodes any supported audio file, resamples to 44100 Hz,
//! and writes a 16-bit PCM WAV file.

use std::env;
use tarang::audio::{self, EncoderConfig, FileDecoder, create_encoder};
use tarang::core::{AudioCodec, Result};
use tarang::demux::{MuxConfig, Muxer, WavMuxer};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: transcode <input> <output.wav>");
        std::process::exit(1);
    }

    let input_path = &args[1];
    let output_path = &args[2];

    // Decode
    println!("Decoding {input_path}...");
    let file = std::fs::File::open(input_path)?;
    let mut decoder = FileDecoder::open(Box::new(file), None)?;
    let audio = decoder.decode_all()?;
    println!(
        "  {} frames, {}Hz, {}ch",
        audio.num_frames, audio.sample_rate, audio.channels
    );

    // Resample to 44100 if needed
    let resampled = if audio.sample_rate != 44100 {
        println!("Resampling {}Hz → 44100Hz...", audio.sample_rate);
        audio::resample(&audio, 44100)?
    } else {
        audio
    };

    // Mix to stereo if needed
    let stereo = if resampled.channels == 1 {
        println!("Mixing mono → stereo...");
        audio::mix_channels(&resampled, audio::ChannelLayout::Stereo)?
    } else {
        resampled
    };

    // Encode as 16-bit PCM
    println!("Encoding as PCM 16-bit...");
    let enc_config = EncoderConfig::builder(AudioCodec::Pcm)
        .sample_rate(44100)
        .channels(stereo.channels)
        .bits_per_sample(16)
        .build();
    let mut encoder = create_encoder(&enc_config)?;
    let packets = encoder.encode(&stereo)?;

    // Mux as WAV
    println!("Writing {output_path}...");
    let output = std::fs::File::create(output_path)?;
    let mux_config = MuxConfig {
        codec: AudioCodec::Pcm,
        sample_rate: 44100,
        channels: stereo.channels,
        bits_per_sample: 16,
    };
    let mut muxer = WavMuxer::new(output, mux_config);
    muxer.write_header()?;
    for packet in &packets {
        muxer.write_packet(packet)?;
    }
    muxer.finalize()?;

    println!("Done.");
    Ok(())
}
