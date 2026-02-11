//! Extract the complete audio track from a media file.
//!
//! Usage:
//!   cargo run --example extract_audio -- <input_file>

use unbundle::{AudioFormat, MediaUnbundler};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    println!("Opening {input_path}...");
    let mut unbundler = MediaUnbundler::open(&input_path)?;

    // Print audio metadata.
    let metadata = unbundler.metadata();
    if let Some(audio_metadata) = &metadata.audio {
        println!(
            "Audio: {} Hz, {} channels, codec: {}, bit rate: {} bps",
            audio_metadata.sample_rate,
            audio_metadata.channels,
            audio_metadata.codec,
            audio_metadata.bit_rate,
        );
    }

    // Extract complete audio to WAV file.
    println!("Extracting audio to output.wav...");
    unbundler.audio().save("output.wav", AudioFormat::Wav)?;
    println!("Saved output.wav");

    // Extract audio to memory as WAV.
    println!("Extracting audio to memory...");
    let audio_bytes = unbundler.audio().extract(AudioFormat::Wav)?;
    println!("Extracted {} bytes of audio data", audio_bytes.len());

    println!("Done!");
    Ok(())
}
