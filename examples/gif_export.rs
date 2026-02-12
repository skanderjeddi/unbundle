//! Export frames as an animated GIF.
//!
//! Usage: `cargo run --features gif --example gif_export -- path/to/video.mp4`

#[cfg(feature = "gif")]
use unbundle::{FrameRange, GifOptions, MediaFile, UnbundleError};

#[cfg(not(feature = "gif"))]
fn main() {
    eprintln!("This example requires the `gif` feature: cargo run --features gif --example gif_export -- <video_path>");
}

#[cfg(feature = "gif")]
fn main() -> Result<(), UnbundleError> {
    let path = std::env::args().nth(1).expect("Usage: gif_export <video_path>");

    let mut unbundler = MediaFile::open(&path)?;
    let meta = unbundler.metadata().clone();
    let video = meta.video.as_ref().expect("no video stream");

    println!(
        "Input: {}x{}, {} frames",
        video.width, video.height, video.frame_count
    );

    // Export first 30 frames as a GIF with 160px width.
    let frame_count = video.frame_count.min(30);
    let config = GifOptions::default().width(160).frame_delay(100);

    let output = "output.gif";
    unbundler
        .video()
        .export_gif(output, FrameRange::Range(0, frame_count as u64), &config)?;

    println!("GIF saved to {output}");

    // Also export to memory.
    let bytes = unbundler.video().export_gif_to_memory(
        FrameRange::Range(0, frame_count as u64),
        &config,
    )?;
    println!("GIF in memory: {} bytes", bytes.len());

    Ok(())
}
