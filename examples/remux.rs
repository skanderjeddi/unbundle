//! Lossless container format conversion (remuxing).
//!
//! Usage:
//!   cargo run --example remux -- <input_file> <output_file>
//!
//! Example:
//!   cargo run --example remux -- input.mkv output.mp4

use std::error::Error;

use unbundle::Remuxer;

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mkv".to_string());
    let output_path = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "output.mp4".to_string());

    println!("Remuxing {input_path} → {output_path}...");

    // Full remux (video + audio + subtitles).
    Remuxer::new(&input_path, &output_path)?.run()?;
    println!("Saved {output_path}");

    // Remux without subtitles.
    let no_subs_path = format!(
        "{}_nosubs.{}",
        output_path.rsplit_once('.').map(|(s, _)| s).unwrap_or(&output_path),
        output_path.rsplit_once('.').map(|(_, e)| e).unwrap_or("mp4"),
    );
    Remuxer::new(&input_path, &no_subs_path)?
        .exclude_subtitles()
        .run()?;
    println!("Saved {no_subs_path} (no subtitles)");

    // Remux audio only (no video, no subtitles).
    let audio_only_path = format!(
        "{}_audio.{}",
        output_path.rsplit_once('.').map(|(s, _)| s).unwrap_or(&output_path),
        output_path.rsplit_once('.').map(|(_, e)| e).unwrap_or("mp4"),
    );
    Remuxer::new(&input_path, &audio_only_path)?
        .exclude_video()
        .exclude_subtitles()
        .run()?;
    println!("Saved {audio_only_path} (audio only)");

    println!("Done!");
    Ok(())
}
