//! Compute an audio fingerprint and compare two files.
//!
//! Usage:
//!   cargo run --example fingerprint -- file.mp3              # print fingerprint
//!   cargo run --example fingerprint -- file1.mp3 file2.mp3   # compare similarity

use std::env;
use tarang::ai::{FingerprintConfig, compute_fingerprint, fingerprint_match};
use tarang::audio::FileDecoder;
use tarang::core::Result;

fn load_and_fingerprint(path: &str) -> Result<tarang::ai::AudioFingerprint> {
    let file = std::fs::File::open(path)?;
    let mut decoder = FileDecoder::open(Box::new(file), None)?;
    let audio = decoder.decode_all()?;

    // Resample to 16kHz mono for fingerprinting
    let resampled = tarang::audio::resample(&audio, 16000)?;
    let mono = tarang::audio::mix_channels(&resampled, tarang::audio::ChannelLayout::Mono)?;

    compute_fingerprint(&mono, &FingerprintConfig::default())
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    match args.len() {
        2 => {
            let fp = load_and_fingerprint(&args[1])?;
            println!("File: {}", args[1]);
            println!("Fingerprint: {} hashes", fp.hashes.len());
            println!("Duration: {:.1}s", fp.duration_secs);
            // Print first 10 hashes as hex
            for (i, hash) in fp.hashes.iter().take(10).enumerate() {
                println!("  [{i:3}] {hash:08x}");
            }
            if fp.hashes.len() > 10 {
                println!("  ... ({} more)", fp.hashes.len() - 10);
            }
        }
        3 => {
            println!("Computing fingerprints...");
            let fp1 = load_and_fingerprint(&args[1])?;
            let fp2 = load_and_fingerprint(&args[2])?;
            let similarity = fingerprint_match(&fp1, &fp2);
            println!("File 1: {} ({} hashes)", args[1], fp1.hashes.len());
            println!("File 2: {} ({} hashes)", args[2], fp2.hashes.len());
            println!("Similarity: {:.1}%", similarity * 100.0);
        }
        _ => {
            eprintln!("Usage:");
            eprintln!("  fingerprint <file>           # print fingerprint");
            eprintln!("  fingerprint <file1> <file2>  # compare similarity");
            std::process::exit(1);
        }
    }

    Ok(())
}
