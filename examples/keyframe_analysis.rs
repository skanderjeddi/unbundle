//! Analyze keyframes and GOP structure.
//!
//! Usage: `cargo run --example keyframe_analysis -- path/to/video.mp4`

use unbundle::MediaUnbundler;

fn main() -> Result<(), unbundle::UnbundleError> {
    let path = std::env::args().nth(1).expect("Usage: keyframe_analysis <video_path>");

    let mut unbundler = MediaUnbundler::open(&path)?;

    // List keyframes.
    let keyframes = unbundler.video().keyframes()?;
    println!("Keyframes ({} total):", keyframes.len());
    for (i, kf) in keyframes.iter().take(20).enumerate() {
        println!(
            "  [{i}] packet={} pts={:?} timestamp={:?} size={} bytes",
            kf.packet_number, kf.pts, kf.timestamp, kf.size
        );
    }
    if keyframes.len() > 20 {
        println!("  ... ({} more)", keyframes.len() - 20);
    }

    // GOP analysis.
    let gop = unbundler.video().analyze_gops()?;
    println!("\nGOP Analysis:");
    println!("  Total video packets: {}", gop.total_video_packets);
    println!("  Number of GOPs: {}", gop.gop_sizes.len());
    println!("  Average GOP size: {:.1}", gop.average_gop_size);
    println!("  Min GOP size: {}", gop.min_gop_size);
    println!("  Max GOP size: {}", gop.max_gop_size);

    Ok(())
}
