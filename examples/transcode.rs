//! Transcode audio between formats.
//!
//! Usage: `cargo run --features transcode --example transcode -- path/to/video.mp4`

#[cfg(feature = "transcode")]
use std::time::Duration;

#[cfg(feature = "transcode")]
use unbundle::{AudioFormat, MediaFile, Transcoder, UnbundleError};

#[cfg(not(feature = "transcode"))]
fn main() {
    eprintln!(
        "This example requires the `transcode` feature: cargo run --features transcode --example transcode -- <video_path>"
    );
}

#[cfg(feature = "transcode")]
fn main() -> Result<(), UnbundleError> {
    let path = std::env::args()
        .nth(1)
        .expect("Usage: transcode <video_path>");

    let mut unbundler = MediaFile::open(&path)?;

    // Transcode full audio to MP3 in memory.
    let mp3_bytes = Transcoder::new(&mut unbundler)
        .format(AudioFormat::Mp3)
        .run_to_memory()?;

    println!("Transcoded to MP3: {} bytes", mp3_bytes.len());

    // Transcode a range to WAV file.
    let output = "transcoded_segment.wav";
    Transcoder::new(&mut unbundler)
        .format(AudioFormat::Wav)
        .start(Duration::from_secs(1))
        .end(Duration::from_secs(3))
        .run(output)?;

    println!("Transcoded segment saved to {output}");
    std::fs::remove_file(output).ok();

    Ok(())
}
