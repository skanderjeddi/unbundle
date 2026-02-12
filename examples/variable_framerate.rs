//! Detect whether a video uses variable frame rate.
//!
//! Usage: `cargo run --example variable_framerate -- path/to/video.mp4`

use unbundle::{MediaFile, UnbundleError};

fn main() -> Result<(), UnbundleError> {
    let path = std::env::args()
        .nth(1)
        .expect("Usage: variable_framerate <video_path>");

    let mut unbundler = MediaFile::open(&path)?;
    let analysis = unbundler.video().analyze_variable_framerate()?;

    println!("VFR Analysis for: {path}");
    println!("  Is VFR: {}", analysis.is_vfr);
    println!(
        "  Mean frame duration: {:.4} ms",
        analysis.mean_frame_duration * 1000.0
    );
    println!(
        "  Std deviation: {:.4} ms",
        analysis.frame_duration_stddev * 1000.0
    );
    println!(
        "  FPS range: {:.2} – {:.2}",
        analysis.min_fps, analysis.max_fps
    );
    println!("  Mean FPS: {:.2}", analysis.mean_fps);
    println!("  Frames analyzed: {}", analysis.frames_analyzed);

    if analysis.is_vfr {
        println!("\n  ⚠ This video has variable frame rate (VFR).");
    } else {
        println!("\n  ✓ This video has constant frame rate (CFR).");
    }

    Ok(())
}
