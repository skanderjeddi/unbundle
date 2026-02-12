//! Analyze audio loudness (peak, RMS, dBFS).
//!
//! Usage: `cargo run --features loudness --example loudness -- path/to/video.mp4`

#[cfg(feature = "loudness")]
use unbundle::{MediaFile, UnbundleError};

#[cfg(not(feature = "loudness"))]
fn main() {
    eprintln!("This example requires the `loudness` feature: cargo run --features loudness --example loudness -- <video_path>");
}

#[cfg(feature = "loudness")]
fn main() -> Result<(), UnbundleError> {
    let path = std::env::args().nth(1).expect("Usage: loudness <video_path>");

    let mut unbundler = MediaFile::open(&path)?;
    let info = unbundler.audio().analyze_loudness()?;

    println!("Loudness Analysis for: {path}");
    println!("  Peak: {:.4} ({:.1} dBFS)", info.peak, info.peak_dbfs);
    println!("  RMS:  {:.4} ({:.1} dBFS)", info.rms, info.rms_dbfs);
    println!("  Duration: {:.2}s", info.duration.as_secs_f64());
    println!("  Total samples: {}", info.total_samples);

    Ok(())
}
