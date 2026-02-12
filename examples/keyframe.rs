//! Analyze keyframes and Group of Pictures structure.
//!
//! Usage: `cargo run --example keyframe -- path/to/video.mp4`

use unbundle::{MediaFile, UnbundleError};

fn main() -> Result<(), UnbundleError> {
    let path = std::env::args()
        .nth(1)
        .expect("Usage: keyframe <video_path>");

    let mut unbundler = MediaFile::open(&path)?;

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

    // Group of Pictures analysis.
    let group_of_pictures = unbundler.video().analyze_group_of_pictures()?;
    println!("\nGroup of Pictures Analysis:");
    println!(
        "  Total video packets: {}",
        group_of_pictures.total_video_packets
    );
    println!(
        "  Number of Group of Pictures sequences: {}",
        group_of_pictures.group_of_pictures_sizes.len()
    );
    println!(
        "  Average Group of Pictures size: {:.1}",
        group_of_pictures.average_group_of_pictures_size
    );
    println!(
        "  Minimum Group of Pictures size: {}",
        group_of_pictures.min_group_of_pictures_size
    );
    println!(
        "  Maximum Group of Pictures size: {}",
        group_of_pictures.max_group_of_pictures_size
    );

    Ok(())
}
