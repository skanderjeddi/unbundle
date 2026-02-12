//! Hardware-accelerated frame extraction example (feature = "hardware").
//!
//! Usage:
//!   cargo run --features=hardware --example hardware_acceleration -- <input_file>

use std::error::Error;

use unbundle::{ExtractOptions, FrameRange, HardwareAccelerationMode, MediaFile};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    // List available hardware decoders on this system.
    let devices = unbundle::hardware_acceleration::available_hardware_devices();
    println!("Available HW devices: {devices:?}");

    println!("Opening {input_path}...");
    let mut unbundler = MediaFile::open(&input_path)?;

    // Use Auto mode — the library will pick the best available device
    // and fall back to software decoding if none is found.
    let config = ExtractOptions::new().with_hardware_acceleration(HardwareAccelerationMode::Auto);

    println!("Extracting 5 frames with HW accel (Auto mode)...");
    let frames = unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 4), &config)?;

    println!("Extracted {} frames", frames.len());
    if let Some(first) = frames.first() {
        first.save("hw_frame.png")?;
        println!("Saved hw_frame.png ({}x{})", first.width(), first.height(),);
    }

    println!("Done!");
    Ok(())
}
