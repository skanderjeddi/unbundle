//! Inspect raw packets without decoding.
//!
//! Usage: `cargo run --example packet_inspect -- path/to/video.mp4`

use unbundle::MediaUnbundler;

fn main() -> Result<(), unbundle::UnbundleError> {
    let path = std::env::args().nth(1).expect("Usage: packet_inspect <video_path>");

    let mut unbundler = MediaUnbundler::open(&path)?;

    println!("Packet-level inspection of: {path}");
    println!("---");

    let mut count = 0usize;
    let mut total_size = 0usize;
    let mut keyframe_count = 0usize;

    for pkt in unbundler.packet_iter()? {
        let pkt = pkt?;
        count += 1;
        total_size += pkt.size;
        if pkt.is_keyframe {
            keyframe_count += 1;
        }

        // Print first 10 packets for demonstration.
        if count <= 10 {
            println!(
                "  stream={} pts={:?} dts={:?} size={} keyframe={}",
                pkt.stream_index, pkt.pts, pkt.dts, pkt.size, pkt.is_keyframe
            );
        }
    }

    if count > 10 {
        println!("  ... ({} more packets)", count - 10);
    }

    println!("---");
    println!("Total packets: {count}");
    println!("Total size: {total_size} bytes");
    println!("Keyframe packets: {keyframe_count}");

    Ok(())
}
