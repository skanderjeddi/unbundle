//! Extract a specific audio segment from a media file.
//!
//! Usage:
//!   cargo run --example extract_audio_segment -- <input_file> <start_secs> <end_secs>

use std::error::Error;
use std::time::Duration;

use unbundle::{AudioFormat, MediaUnbundler};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());
    let start_seconds: f64 = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(30.0);
    let end_seconds: f64 = std::env::args()
        .nth(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(60.0);

    let start = Duration::from_secs_f64(start_seconds);
    let end = Duration::from_secs_f64(end_seconds);

    println!("Opening {input_path}...");
    let mut unbundler = MediaUnbundler::open(&input_path)?;

    let metadata = unbundler.metadata();
    println!("Media duration: {:?}", metadata.duration);

    if let Some(audio_metadata) = &metadata.audio {
        println!(
            "Audio: {} Hz, {} channels, codec: {}",
            audio_metadata.sample_rate, audio_metadata.channels, audio_metadata.codec,
        );
    }

    // Save segment as MP3 to file.
    let output_path = "audio_segment.mp3";
    println!(
        "Extracting audio segment {:.1}s - {:.1}s to {output_path}...",
        start_seconds, end_seconds,
    );
    unbundler
        .audio()
        .save_range(output_path, start, end, AudioFormat::Mp3)?;
    println!("Saved {output_path}");

    // Also demonstrate extracting a segment to memory.
    println!("Extracting segment to memory as WAV...");
    let audio_bytes = unbundler
        .audio()
        .extract_range(start, end, AudioFormat::Wav)?;
    println!("Extracted {} bytes of audio data", audio_bytes.len());

    println!("Done!");
    Ok(())
}
