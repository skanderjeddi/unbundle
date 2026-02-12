//! Transcoding integration tests.
//!
//! Requires the `transcode` feature and test fixtures.

use std::path::Path;
use std::time::Duration;

use unbundle::{AudioFormat, MediaUnbundler, Transcoder};

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn transcode_to_memory_wav() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let bytes = Transcoder::new(&mut unbundler)
        .format(AudioFormat::Wav)
        .run_to_memory()
        .expect("transcode to memory");

    assert!(!bytes.is_empty());
    assert_eq!(&bytes[..4], b"RIFF", "expected WAV RIFF header");
}

#[test]
fn transcode_to_memory_mp3() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let bytes = Transcoder::new(&mut unbundler)
        .format(AudioFormat::Mp3)
        .run_to_memory()
        .expect("transcode to memory mp3");

    assert!(!bytes.is_empty());
}

#[test]
fn transcode_to_file() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let output = "tests/fixtures/test_transcode_output.wav";
    let mut unbundler = MediaUnbundler::open(path).expect("open");
    Transcoder::new(&mut unbundler)
        .format(AudioFormat::Wav)
        .run(output)
        .expect("transcode to file");

    assert!(Path::new(output).exists());
    let data = std::fs::read(output).expect("read");
    assert_eq!(&data[..4], b"RIFF");
    std::fs::remove_file(output).ok();
}

#[test]
fn transcode_with_range() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let full = Transcoder::new(&mut unbundler)
        .format(AudioFormat::Wav)
        .run_to_memory()
        .expect("full");

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let partial = Transcoder::new(&mut unbundler)
        .format(AudioFormat::Wav)
        .start(Duration::from_secs(1))
        .end(Duration::from_secs(3))
        .run_to_memory()
        .expect("partial");

    assert!(
        partial.len() < full.len(),
        "partial ({}) should be smaller than full ({})",
        partial.len(),
        full.len()
    );
}
