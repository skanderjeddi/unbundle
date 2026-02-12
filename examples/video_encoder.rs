//! Write extracted frames to a new video file.
//!
//! Usage: `cargo run --features encode --example video_encoder -- path/to/video.mp4`

#[cfg(feature = "encode")]
use unbundle::{
    FrameRange, MediaFile, UnbundleError, VideoCodec, VideoEncoder, VideoEncoderOptions,
};

#[cfg(not(feature = "encode"))]
fn main() {
    eprintln!(
        "This example requires the `encode` feature: cargo run --features encode --example video_encoder -- <video_path>"
    );
}

#[cfg(feature = "encode")]
fn main() -> Result<(), UnbundleError> {
    let path = std::env::args()
        .nth(1)
        .expect("Usage: video_encoder <video_path>");

    let mut unbundler = MediaFile::open(&path)?;

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
    let config = VideoEncoderOptions::default()
        .fps(24)
        .codec(VideoCodec::H264);

    VideoEncoder::new(config).write(output, &frames)?;

    let size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    println!("Wrote {output} ({size} bytes)");
    std::fs::remove_file(output).ok();

    Ok(())
}
