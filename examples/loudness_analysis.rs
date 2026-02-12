//! Analyze audio loudness (peak, RMS, dBFS).
//!
//! Usage: `cargo run --features loudness --example loudness_analysis -- path/to/video.mp4`

use unbundle::MediaUnbundler;

fn main() -> Result<(), unbundle::UnbundleError> {
    let path = std::env::args().nth(1).expect("Usage: loudness_analysis <video_path>");

    let mut unbundler = MediaUnbundler::open(&path)?;
    let info = unbundler.audio().analyze_loudness()?;

    println!("Loudness Analysis for: {path}");
    println!("  Peak: {:.4} ({:.1} dBFS)", info.peak, info.peak_dbfs);
    println!("  RMS:  {:.4} ({:.1} dBFS)", info.rms, info.rms_dbfs);
    println!("  Duration: {:.2}s", info.duration.as_secs_f64());
    println!("  Total samples: {}", info.total_samples);

    Ok(())
}
