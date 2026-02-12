//! Generate an audio waveform visualization.
//!
//! Usage: `cargo run --features waveform --example waveform -- path/to/video.mp4`

#[cfg(feature = "waveform")]
use unbundle::{MediaFile, UnbundleError, WaveformOptions};

#[cfg(not(feature = "waveform"))]
fn main() {
    eprintln!("This example requires the `waveform` feature: cargo run --features waveform --example waveform -- <video_path>");
}

#[cfg(feature = "waveform")]
fn main() -> Result<(), UnbundleError> {
    let path = std::env::args().nth(1).expect("Usage: waveform <video_path>");

    let mut unbundler = MediaFile::open(&path)?;

    let config = WaveformOptions {
        bins: 80,
        ..Default::default()
    };

    let waveform = unbundler.audio().generate_waveform(&config)?;

    println!("Waveform for: {path}");
    println!(
        "  Duration: {:.2}s | Sample rate: {} | Samples: {}",
        waveform.duration.as_secs_f64(),
        waveform.sample_rate,
        waveform.total_samples
    );
    println!("  Bins: {}", waveform.bins.len());
    println!();

    // Simple ASCII waveform.
    let max_height = 20;
    for bin in &waveform.bins {
        let bar_len = (bin.rms * max_height as f32) as usize;
        let bar: String = "█".repeat(bar_len);
        print!("{bar}");
        let pad: String = " ".repeat(max_height - bar_len);
        print!("{pad}|");
    }
    println!();

    Ok(())
}
