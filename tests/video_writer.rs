//! Video writer integration tests.
//!
//! Requires the `video-writer` feature and test fixtures.

use std::path::Path;

use unbundle::{FrameRange, MediaUnbundler, VideoCodec, VideoWriter, VideoWriterConfig};

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn write_frames_to_mp4() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let frames = unbundler
        .video()
        .frames(FrameRange::Range(0, 5))
        .expect("extract frames");

    let output = "tests/fixtures/test_writer_output.mp4";
    let config = VideoWriterConfig::default().fps(10);
    let result = VideoWriter::new(config).write(output, &frames);

    // Skip if the H264 encoder is not available on this platform.
    if let Err(ref e) = result {
        let msg = format!("{e}");
        if msg.contains("cannot open encoder") || msg.contains("codec") {
            eprintln!("Skipping: H264 encoder not available ({msg})");
            return;
        }
    }
    result.expect("write video");

    assert!(Path::new(output).exists());
    let file_size = std::fs::metadata(output).unwrap().len();
    assert!(file_size > 0, "output file should be non-empty");
    std::fs::remove_file(output).ok();
}

#[test]
fn write_frames_with_resolution() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let frames = unbundler
        .video()
        .frames(FrameRange::Range(0, 3))
        .expect("extract frames");

    let output = "tests/fixtures/test_writer_resized.mp4";
    let config = VideoWriterConfig::default()
        .fps(5)
        .resolution(160, 120)
        .codec(VideoCodec::H264);
    let result = VideoWriter::new(config).write(output, &frames);

    // Skip if the H264 encoder is not available on this platform.
    if let Err(ref e) = result {
        let msg = format!("{e}");
        if msg.contains("cannot open encoder") || msg.contains("codec") {
            eprintln!("Skipping: H264 encoder not available ({msg})");
            return;
        }
    }
    result.expect("write resized video");

    assert!(Path::new(output).exists());
    std::fs::remove_file(output).ok();
}

#[test]
fn write_empty_frames_returns_error() {
    let output = "tests/fixtures/test_writer_empty.mp4";
    let config = VideoWriterConfig::default();
    let result = VideoWriter::new(config).write(output, &[]);

    assert!(result.is_err(), "should error on empty frames");
}

#[test]
fn video_writer_config_builder() {
    let config = VideoWriterConfig::default()
        .fps(24)
        .resolution(1920, 1080)
        .codec(VideoCodec::H265)
        .crf(18)
        .bitrate(5_000_000);

    assert_eq!(config.fps, 24);
    assert_eq!(config.width, Some(1920));
    assert_eq!(config.height, Some(1080));
    assert_eq!(config.codec, VideoCodec::H265);
    assert_eq!(config.crf, Some(18));
    assert_eq!(config.bitrate, Some(5_000_000));
}
