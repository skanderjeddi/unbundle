//! Extract video frames from a media file.
//!
//! Usage:
//!   cargo run --example extract_frames -- <input_file>

use std::error::Error;
use std::time::Duration;

use unbundle::{FrameRange, MediaUnbundler};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    println!("Opening {input_path}...");
    let mut unbundler = MediaUnbundler::open(&input_path)?;

    // Print video metadata.
    let metadata = unbundler.metadata();
    if let Some(video_metadata) = &metadata.video {
        println!(
            "Video: {}x{}, {:.2} fps, {} frames",
            video_metadata.width,
            video_metadata.height,
            video_metadata.frames_per_second,
            video_metadata.frame_count,
        );
    }

    // Extract the first frame.
    println!("Extracting first frame...");
    let frame = unbundler.video().frame(0)?;
    frame.save("frame_first.png")?;
    println!("Saved frame_first.png");

    // Extract a frame at the 5-second mark.
    println!("Extracting frame at 5 seconds...");
    let frame = unbundler.video().frame_at(Duration::from_secs(5))?;
    frame.save("frame_5s.png")?;
    println!("Saved frame_5s.png");

    // Extract every 30th frame.
    println!("Extracting every 30th frame...");
    let frames = unbundler.video().frames(FrameRange::Interval(30))?;
    for (index, frame) in frames.iter().enumerate() {
        let filename = format!("frame_interval_{index}.png");
        frame.save(&filename)?;
    }
    println!("Saved {} interval frames", frames.len());

    // Extract specific frames.
    println!("Extracting specific frames [0, 50, 100]...");
    let frames = unbundler
        .video()
        .frames(FrameRange::Specific(vec![0, 50, 100]))?;
    for (index, frame) in frames.iter().enumerate() {
        let filename = format!("frame_specific_{index}.png");
        frame.save(&filename)?;
    }
    println!("Saved {} specific frames", frames.len());

    println!("Done!");
    Ok(())
}
