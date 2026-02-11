//! Extract subtitles from a media file.
//!
//! Usage:
//!   cargo run --example subtitles -- <input_file>

use std::error::Error;

use unbundle::{MediaUnbundler, SubtitleFormat};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mkv".to_string());

    println!("Opening {input_path}...");
    let mut unbundler = MediaUnbundler::open(&input_path)?;

    let metadata = unbundler.metadata();
    if let Some(tracks) = &metadata.subtitle_tracks {
        println!("Found {} subtitle track(s)", tracks.len());
        for track in tracks {
            println!(
                "  Track {}: codec={}, language={}",
                track.track_index,
                track.codec,
                track.language.as_deref().unwrap_or("unknown"),
            );
        }
    } else {
        println!("No subtitle tracks found");
        return Ok(());
    }

    // Extract subtitle entries from the default track.
    println!("\nExtracting subtitles...");
    let entries = unbundler.subtitle().extract()?;
    println!("Found {} subtitle entries:", entries.len());
    for entry in entries.iter().take(10) {
        println!(
            "  [{:?} → {:?}] {}",
            entry.start_time, entry.end_time, entry.text,
        );
    }
    if entries.len() > 10 {
        println!("  ... and {} more", entries.len() - 10);
    }

    // Save as SRT.
    unbundler
        .subtitle()
        .save("output.srt", SubtitleFormat::Srt)?;
    println!("\nSaved output.srt");

    // Save as WebVTT.
    unbundler
        .subtitle()
        .save("output.vtt", SubtitleFormat::WebVtt)?;
    println!("Saved output.vtt");

    // Extract plain text.
    let text = unbundler.subtitle().extract_text(SubtitleFormat::Raw)?;
    println!("\nPlain text ({} chars):", text.len());
    for line in text.lines().take(5) {
        println!("  {line}");
    }

    println!("\nDone!");
    Ok(())
}
