//! Probe a media file and display its metadata.
//!
//! Usage: cargo run --example probe -- path/to/file.mp3

use std::env;

fn main() -> tarang::core::Result<()> {
    let path = env::args().nth(1).expect("usage: probe <file>");
    let file = std::fs::File::open(&path)?;
    let info = tarang::audio::probe_audio(file)?;

    println!("File:     {path}");
    println!("Format:   {}", info.format);
    if let Some(d) = info.duration {
        println!("Duration: {:.1}s", d.as_secs_f64());
    }
    println!("Streams:  {}", info.streams.len());

    for (i, stream) in info.streams.iter().enumerate() {
        match stream {
            tarang::core::StreamInfo::Audio(a) => {
                println!(
                    "  [{i}] Audio: {} {}Hz {}ch",
                    a.codec, a.sample_rate, a.channels
                );
            }
            tarang::core::StreamInfo::Video(v) => {
                println!(
                    "  [{i}] Video: {} {}x{} {:.1}fps",
                    v.codec, v.width, v.height, v.frame_rate
                );
            }
            tarang::core::StreamInfo::Subtitle { language } => {
                println!(
                    "  [{i}] Subtitle: {}",
                    language.as_deref().unwrap_or("unknown")
                );
            }
            _ => println!("  [{i}] Unknown stream"),
        }
    }

    // Show metadata tags
    if !info.metadata.is_empty() {
        println!("Metadata:");
        for (key, value) in &info.metadata {
            println!("  {key}: {value}");
        }
    }

    Ok(())
}
