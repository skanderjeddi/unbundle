//! Async frame and audio extraction example (feature = "async-tokio").
//!
//! Usage:
//!   cargo run --features=async-tokio --example async_extraction -- <input_file>

use std::error::Error;

use tokio_stream::StreamExt;
use unbundle::{AudioFormat, ExtractionConfig, FrameRange, MediaUnbundler};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    println!("Opening {input_path}...");
    let mut unbundler = MediaUnbundler::open(&input_path)?;

    // --- Async frame stream ---------------------------------------------------
    let range = FrameRange::Range(0, 29);
    let config = ExtractionConfig::new();

    println!("Streaming frames 0–29...");
    let mut stream = unbundler.video().frame_stream(range, config)?;

    let mut count = 0u64;
    while let Some(result) = stream.next().await {
        let (frame_number, image) = result?;
        if count == 0 {
            image.save("async_first_frame.png")?;
            println!(
                "Saved async_first_frame.png ({}x{})",
                image.width(),
                image.height(),
            );
        }
        count += 1;
        print!("\rProcessed frame {frame_number} ({count} total)");
    }
    println!();

    // --- Async audio extraction -----------------------------------------------
    println!("Extracting audio asynchronously...");
    let audio_bytes = unbundler
        .audio()
        .extract_async(AudioFormat::Wav, ExtractionConfig::new())?
        .await?;
    println!("Got {} bytes of WAV audio", audio_bytes.len());

    println!("Done!");
    Ok(())
}
