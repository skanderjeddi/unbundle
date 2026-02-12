//! Write extracted frames to a new video file.
//!
//! Usage: `cargo run --features video-writer --example write_video -- path/to/video.mp4`

use unbundle::{FrameRange, MediaUnbundler, VideoCodec, VideoWriter, VideoWriterConfig};

fn main() -> Result<(), unbundle::UnbundleError> {
    let path = std::env::args().nth(1).expect("Usage: write_video <video_path>");

    let mut unbundler = MediaUnbundler::open(&path)?;

    // Extract first 30 frames.
    let frame_count = unbundler
        .metadata()
        .video
        .as_ref()
        .expect("no video stream")
        .frame_count
        .min(30);

    println!("Extracting {frame_count} frames...");
    let frames = unbundler
        .video()
        .frames(FrameRange::Range(0, frame_count as u64))?;

    // Write to a new MP4 at 24 fps.
    let output = "output_written.mp4";
    let config = VideoWriterConfig::default()
        .fps(24)
        .codec(VideoCodec::H264);

    VideoWriter::new(config).write(output, &frames)?;

    let size = std::fs::metadata(output)
        .map(|m| m.len())
        .unwrap_or(0);
    println!("Wrote {output} ({size} bytes)");
    std::fs::remove_file(output).ok();

    Ok(())
}
