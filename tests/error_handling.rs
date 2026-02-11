//! Error handling integration tests.
//!
//! These tests verify that meaningful errors are returned for various
//! failure conditions.

use std::{path::Path, time::Duration};

use unbundle::{AudioFormat, MediaUnbundler};

#[test]
fn open_nonexistent_file() {
    let result = MediaUnbundler::open("this_file_does_not_exist.mp4");
    assert!(result.is_err());

    let error_message = result.unwrap_err().to_string();
    assert!(
        error_message.contains("Failed to open media file"),
        "Error message should mention file open failure: {error_message}",
    );
}

#[test]
fn open_invalid_file() {
    // Create a temporary file with garbage content.
    let temporary_directory = tempfile::tempdir().expect("Failed to create temp dir");
    let invalid_file_path = temporary_directory.path().join("invalid.mp4");
    std::fs::write(&invalid_file_path, b"this is not a media file")
        .expect("Failed to write invalid file");

    let result = MediaUnbundler::open(&invalid_file_path);
    assert!(result.is_err(), "Expected error for invalid media file");
}

#[test]
fn frame_out_of_range() {
    let path = "tests/fixtures/sample_video.mp4";
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("Failed to open test video");
    let result = unbundler.video().frame(999_999);
    assert!(result.is_err());

    let error_message = result.unwrap_err().to_string();
    assert!(
        error_message.contains("out of range"),
        "Error message should mention out of range: {error_message}",
    );
}

#[test]
fn invalid_timestamp() {
    let path = "tests/fixtures/sample_video.mp4";
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("Failed to open test video");
    // 1 hour is way beyond a 5-second video.
    let result = unbundler.video().frame_at(Duration::from_secs(3600));
    assert!(result.is_err());

    let error_message = result.unwrap_err().to_string();
    assert!(
        error_message.contains("Invalid timestamp"),
        "Error should mention invalid timestamp: {error_message}",
    );
}

#[test]
fn no_video_stream_error() {
    let path = "tests/fixtures/sample_audio_only.mp4";
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("Failed to open audio-only file");
    let result = unbundler.video().frame(0);
    assert!(result.is_err());

    let error_message = result.unwrap_err().to_string();
    assert!(
        error_message.contains("No video stream"),
        "Error should mention no video stream: {error_message}",
    );
}

#[test]
fn no_audio_stream_error() {
    let path = "tests/fixtures/sample_video_only.mp4";
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("Failed to open video-only file");
    let result = unbundler.audio().extract(AudioFormat::Wav);
    assert!(result.is_err());

    let error_message = result.unwrap_err().to_string();
    assert!(
        error_message.contains("No audio stream"),
        "Error should mention no audio stream: {error_message}",
    );
}

#[test]
fn invalid_audio_range_timestamps() {
    let path = "tests/fixtures/sample_video.mp4";
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("Failed to open test video");
    // End time exceeds media duration.
    let result = unbundler.audio().extract_range(
        Duration::from_secs(0),
        Duration::from_secs(3600),
        AudioFormat::Wav,
    );
    assert!(result.is_err());
}
