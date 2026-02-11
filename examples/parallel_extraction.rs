//! Parallel frame extraction example (feature = "parallel").
//!
//! Usage:
//!   cargo run --features=parallel --example parallel_extraction -- <input_file>

use std::error::Error;
use std::time::Instant;

use unbundle::{ExtractionConfig, FrameRange, MediaUnbundler};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    println!("Opening {input_path}...");
    let mut unbundler = MediaUnbundler::open(&input_path)?;

    let metadata = unbundler.metadata();
    let total_frames = metadata
        .video
        .as_ref()
        .map(|v| v.frame_count)
        .unwrap_or(0);

    if total_frames < 100 {
        println!("Video has only {total_frames} frames — extracting all of them.");
    }

    let frame_count = total_frames.min(100);
    let range = FrameRange::Range(0, frame_count.saturating_sub(1));
    let config = ExtractionConfig::new();

    println!("Extracting {frame_count} frames in parallel...");
    let start = Instant::now();
    let frames = unbundler.video().frames_parallel(range, &config)?;
    let elapsed = start.elapsed();

    println!(
        "Extracted {} frames in {elapsed:.2?} ({:.1} fps)",
        frames.len(),
        frames.len() as f64 / elapsed.as_secs_f64(),
    );

    if let Some(first) = frames.first() {
        first.save("parallel_first.png")?;
        println!("Saved parallel_first.png");
    }

    println!("Done!");
    Ok(())
}
