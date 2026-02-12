//! Demonstrate progress reporting and cancellation during extraction.
//!
//! Usage:
//!   cargo run --example progress -- <input_file>

use std::error::Error;
use std::sync::Arc;

use unbundle::{
    CancellationToken, ExtractOptions, FrameRange, MediaFile, ProgressCallback, ProgressInfo,
};

/// Simple progress callback that prints to stdout.
struct PrintProgress;

impl ProgressCallback for PrintProgress {
    fn on_progress(&self, info: &ProgressInfo) {
        let pct = info
            .percentage
            .map_or("??".to_string(), |p| format!("{p:.1}"));
        let remaining = info
            .estimated_remaining
            .map_or("???".to_string(), |r| format!("{:.1}s", r.as_secs_f64()));
        println!(
            "[{:?}] {}/{} ({pct}%) elapsed={:.1}s remaining={remaining}",
            info.operation,
            info.current,
            info.total.map_or("?".to_string(), |t| t.to_string()),
            info.elapsed.as_secs_f64(),
        );
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    let mut unbundler = MediaFile::open(&input_path)?;

    // ── Progress callback ──────────────────────────────────────────
    println!("Extracting frames with progress reporting...");
    let config = ExtractOptions::new()
        .with_progress(Arc::new(PrintProgress))
        .with_batch_size(5);

    let frames = unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 29), &config)?;
    println!("Extracted {} frames\n", frames.len());

    // ── Cancellation token ─────────────────────────────────────────
    println!("Demonstrating cancellation...");
    let token = CancellationToken::new();
    let cancel_config = ExtractOptions::new().with_cancellation(token.clone());

    // Cancel immediately to demonstrate the mechanism.
    token.cancel();

    let result = unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 99), &cancel_config);

    match result {
        Err(ref e) if e.to_string().contains("ancelled") => {
            println!("Operation was cancelled as expected.");
        }
        Err(e) => println!("Unexpected error: {e}"),
        Ok(frames) => println!("Got {} frames (cancel was too late)", frames.len()),
    }

    println!("\nDone!");
    Ok(())
}
