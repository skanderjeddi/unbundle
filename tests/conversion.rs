//! Container format conversion (remuxing) integration tests.
//!
//! Tests require fixture files from `tests/fixtures/generate_fixtures.sh`.

use std::path::Path;

use unbundle::{MediaUnbundler, Remuxer};

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

fn sample_mkv_path() -> &'static str {
    "tests/fixtures/sample_video.mkv"
}

#[test]
fn remux_mp4_to_mkv() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let tmp = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let output_path = tmp.path().with_extension("mkv");

    Remuxer::new(path, &output_path)
        .expect("Failed to create remuxer")
        .run()
        .expect("Failed to remux");

    // Verify the output file can be opened and has video + audio.
    let unbundler = MediaUnbundler::open(&output_path).expect("Failed to open remuxed file");
    let metadata = unbundler.metadata();
    assert!(metadata.video.is_some(), "Remuxed file should have video");
    assert!(metadata.audio.is_some(), "Remuxed file should have audio");

    let _ = std::fs::remove_file(&output_path);
}

#[test]
fn remux_mkv_to_mp4() {
    let path = sample_mkv_path();
    if !Path::new(path).exists() {
        return;
    }

    let tmp = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let output_path = tmp.path().with_extension("mp4");

    Remuxer::new(path, &output_path)
        .expect("Failed to create remuxer")
        .run()
        .expect("Failed to remux");

    let unbundler = MediaUnbundler::open(&output_path).expect("Failed to open remuxed file");
    assert!(unbundler.metadata().video.is_some());

    let _ = std::fs::remove_file(&output_path);
}

#[test]
fn remux_exclude_audio() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let tmp = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let output_path = tmp.path().with_extension("mp4");

    Remuxer::new(path, &output_path)
        .expect("Failed to create remuxer")
        .exclude_audio()
        .run()
        .expect("Failed to remux without audio");

    let unbundler = MediaUnbundler::open(&output_path).expect("Failed to open remuxed file");
    let metadata = unbundler.metadata();
    assert!(metadata.video.is_some(), "Should still have video");
    assert!(metadata.audio.is_none(), "Audio should be excluded");

    let _ = std::fs::remove_file(&output_path);
}

#[test]
fn remux_exclude_video() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let tmp = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let output_path = tmp.path().with_extension("mp4");

    Remuxer::new(path, &output_path)
        .expect("Failed to create remuxer")
        .exclude_video()
        .run()
        .expect("Failed to remux without video");

    let unbundler = MediaUnbundler::open(&output_path).expect("Failed to open remuxed file");
    let metadata = unbundler.metadata();
    assert!(metadata.video.is_none(), "Video should be excluded");
    assert!(metadata.audio.is_some(), "Should still have audio");

    let _ = std::fs::remove_file(&output_path);
}

#[test]
fn remux_nonexistent_input_error() {
    let result = Remuxer::new("this_does_not_exist.mp4", "output.mp4");
    assert!(result.is_err(), "Expected error for nonexistent input");
}
