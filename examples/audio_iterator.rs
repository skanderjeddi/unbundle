//! Iterate over decoded audio samples in a streaming fashion.
//!
//! Usage: `cargo run --example audio_iterator -- path/to/video.mp4`

use unbundle::MediaUnbundler;

fn main() -> Result<(), unbundle::UnbundleError> {
    let path = std::env::args().nth(1).expect("Usage: audio_iterator <video_path>");

    let mut unbundler = MediaUnbundler::open(&path)?;

    println!("Streaming audio samples from: {path}");

    let mut chunk_count = 0usize;
    let mut total_samples = 0usize;

    for chunk in unbundler.audio().sample_iter()? {
        let chunk = chunk?;
        chunk_count += 1;
        total_samples += chunk.samples.len();

        if chunk_count <= 5 {
            println!(
                "  Chunk {}: {} samples @ {}Hz, timestamp={:?}",
                chunk_count,
                chunk.samples.len(),
                chunk.sample_rate,
                chunk.timestamp
            );
        }
    }

    if chunk_count > 5 {
        println!("  ... ({} more chunks)", chunk_count - 5);
    }

    println!("Total: {chunk_count} chunks, {total_samples} samples");

    Ok(())
}
