//! Display media file metadata.
//!
//! Usage:
//!   cargo run --example metadata -- <input_file>

use std::error::Error;

use unbundle::MediaUnbundler;

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    println!("Opening {input_path}...");
    let unbundler = MediaUnbundler::open(&input_path)?;
    let metadata = unbundler.metadata();

    println!();
    println!("=== Media Metadata ===");
    println!("Format:   {}", metadata.format);
    println!("Duration: {:?}", metadata.duration);

    if let Some(video_metadata) = &metadata.video {
        println!();
        println!("--- Video Stream ---");
        println!("  Codec:            {}", video_metadata.codec);
        println!(
            "  Resolution:       {}x{}",
            video_metadata.width, video_metadata.height,
        );
        println!(
            "  Frame rate:       {:.2} fps",
            video_metadata.frames_per_second,
        );
        println!("  Frame count:      {}", video_metadata.frame_count);
    } else {
        println!();
        println!("--- No Video Stream ---");
    }

    if let Some(audio_metadata) = &metadata.audio {
        println!();
        println!("--- Audio Stream ---");
        println!("  Codec:            {}", audio_metadata.codec);
        println!("  Sample rate:      {} Hz", audio_metadata.sample_rate);
        println!("  Channels:         {}", audio_metadata.channels);
        println!("  Bit rate:         {} bps", audio_metadata.bit_rate);
    } else {
        println!();
        println!("--- No Audio Stream ---");
    }

    Ok(())
}
