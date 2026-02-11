//! Hardware-accelerated frame extraction example (feature = "hw-accel").
//!
//! Usage:
//!   cargo run --features=hw-accel --example hw_acceleration -- <input_file>

use std::error::Error;

use unbundle::{ExtractionConfig, FrameRange, HwAccelMode, MediaUnbundler};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    // List available hardware decoders on this system.
    let devices = unbundle::hw_accel::available_hw_devices();
    println!("Available HW devices: {devices:?}");

    println!("Opening {input_path}...");
    let mut unbundler = MediaUnbundler::open(&input_path)?;

    // Use Auto mode — the library will pick the best available device
    // and fall back to software decoding if none is found.
    let config = ExtractionConfig::new().with_hw_accel(HwAccelMode::Auto);

    println!("Extracting 5 frames with HW accel (Auto mode)...");
    let frames = unbundler
        .video()
        .frames_with_config(FrameRange::Range(0, 4), &config)?;

    println!("Extracted {} frames", frames.len());
    if let Some(first) = frames.first() {
        first.save("hw_frame.png")?;
        println!(
            "Saved hw_frame.png ({}x{})",
            first.width(),
            first.height(),
        );
    }

    println!("Done!");
    Ok(())
}
