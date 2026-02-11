//! Configurable pixel format and resolution for frame extraction.
//!
//! Usage:
//!   cargo run --example pixel_formats -- <input_file>

use std::error::Error;

use unbundle::{ExtractionConfig, FrameRange, MediaUnbundler, OutputPixelFormat};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    let mut unbundler = MediaUnbundler::open(&input_path)?;

    // ── RGB8 (default) ─────────────────────────────────────────────
    let config_rgb = ExtractionConfig::new()
        .with_pixel_format(OutputPixelFormat::Rgb8);
    let frames = unbundler
        .video()
        .frames_with_config(FrameRange::Range(0, 0), &config_rgb)?;
    let frame = &frames[0];
    println!("RGB8:  {}x{}, color={:?}", frame.width(), frame.height(), frame.color());

    // ── RGBA8 ──────────────────────────────────────────────────────
    let config_rgba = ExtractionConfig::new()
        .with_pixel_format(OutputPixelFormat::Rgba8);
    let frames = unbundler
        .video()
        .frames_with_config(FrameRange::Range(0, 0), &config_rgba)?;
    let frame = &frames[0];
    println!("RGBA8: {}x{}, color={:?}", frame.width(), frame.height(), frame.color());

    // ── Grayscale ──────────────────────────────────────────────────
    let config_gray = ExtractionConfig::new()
        .with_pixel_format(OutputPixelFormat::Gray8);
    let frames = unbundler
        .video()
        .frames_with_config(FrameRange::Range(0, 0), &config_gray)?;
    let frame = &frames[0];
    println!("Gray8: {}x{}, color={:?}", frame.width(), frame.height(), frame.color());

    // ── Custom resolution ──────────────────────────────────────────
    let config_scaled = ExtractionConfig::new()
        .with_resolution(Some(320), None); // width=320, height auto
    let frames = unbundler
        .video()
        .frames_with_config(FrameRange::Range(0, 0), &config_scaled)?;
    let frame = &frames[0];
    println!(
        "Scaled (w=320, auto h): {}x{}",
        frame.width(),
        frame.height(),
    );

    let config_fixed = ExtractionConfig::new()
        .with_resolution(Some(200), Some(100))
        .with_maintain_aspect_ratio(false);
    let frames = unbundler
        .video()
        .frames_with_config(FrameRange::Range(0, 0), &config_fixed)?;
    let frame = &frames[0];
    println!(
        "Fixed (200x100, no AR): {}x{}",
        frame.width(),
        frame.height(),
    );

    println!("\nDone!");
    Ok(())
}
