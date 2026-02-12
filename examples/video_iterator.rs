//! Lazy frame iteration using `FrameIterator`.
//!
//! Usage:
//!   cargo run --example video_iterator -- <input_file>

use std::error::Error;

use unbundle::{FrameRange, MediaFile};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    let mut unbundler = MediaFile::open(&input_path)?;
    let metadata = unbundler.metadata().clone();
    let frame_count = metadata.video.as_ref().map_or(0, |v| v.frame_count);
    println!("Video has ~{frame_count} frames");

    // ── Basic lazy iteration ───────────────────────────────────────
    println!("\nIterating over first 10 frames lazily...");
    let iter = unbundler.video().frame_iter(FrameRange::Range(0, 9))?;

    for result in iter {
        let (frame_number, image) = result?;
        println!(
            "  Frame {frame_number}: {}x{}",
            image.width(),
            image.height(),
        );
    }

    // ── Early exit ─────────────────────────────────────────────────
    println!("\nDemonstrating early exit (stop after 3 frames)...");
    let iter = unbundler.video().frame_iter(FrameRange::Range(0, 29))?;

    let mut count = 0;
    for result in iter {
        let (frame_number, _image) = result?;
        println!("  Frame {frame_number}");
        count += 1;
        if count >= 3 {
            println!("  (stopping early)");
            break;
        }
    }

    // ── Specific frames ────────────────────────────────────────────
    println!("\nIterating over specific frames...");
    let targets = vec![0, 10, 20, 30];
    let iter = unbundler
        .video()
        .frame_iter(FrameRange::Specific(targets))?;

    for result in iter {
        let (frame_number, image) = result?;
        println!(
            "  Frame {frame_number}: {}x{}",
            image.width(),
            image.height(),
        );
    }

    println!("\nDone!");
    Ok(())
}
