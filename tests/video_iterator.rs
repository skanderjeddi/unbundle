//! FrameIterator integration tests.
//!
//! Tests require fixture files from `tests/fixtures/generate_fixtures.sh`.

use std::path::Path;

use unbundle::{FrameOutputOptions, FrameRange, MediaFile, PixelFormat};

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

fn sample_video_only_path() -> &'static str {
    "tests/fixtures/sample_video_only.mp4"
}

fn sample_audio_only_path() -> &'static str {
    "tests/fixtures/sample_audio_only.mp4"
}

// ── basic iteration ────────────────────────────────────────────────

#[test]
fn frame_iter_range() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let iter = unbundler
        .video()
        .frame_iter(FrameRange::Range(0, 4))
        .expect("Failed to create iterator");

    let results: Vec<_> = iter.collect();
    assert!(!results.is_empty(), "Expected at least one frame");
    for result in &results {
        assert!(result.is_ok(), "Each frame should decode successfully");
    }
}

#[test]
fn frame_iter_yields_correct_frame_numbers() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let iter = unbundler
        .video()
        .frame_iter(FrameRange::Range(0, 4))
        .expect("Failed to create iterator");

    let mut frame_numbers = Vec::new();
    for result in iter {
        let (frame_num, _) = result.expect("Decode error");
        frame_numbers.push(frame_num);
    }

    // Frame numbers should be sorted.
    for window in frame_numbers.windows(2) {
        assert!(window[1] >= window[0], "Frame numbers should be non-decreasing");
    }
}

#[test]
fn frame_iter_specific_frames() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let targets = vec![0, 5, 10];
    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let iter = unbundler
        .video()
        .frame_iter(FrameRange::Specific(targets))
        .expect("Failed to create iterator");

    let results: Vec<_> = iter.collect();
    assert!(!results.is_empty(), "Expected at least one frame");
    for result in &results {
        assert!(result.is_ok());
    }
}

// ── early exit ─────────────────────────────────────────────────────

#[test]
fn frame_iter_early_exit() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let iter = unbundler
        .video()
        .frame_iter(FrameRange::Range(0, 99))
        .expect("Failed to create iterator");

    let mut count = 0u64;
    for result in iter {
        let _ = result.expect("Decode error");
        count += 1;
        if count >= 3 {
            break;
        }
    }

    assert_eq!(count, 3, "Should have yielded exactly 3 frames before break");
}

// ── with_config variant ────────────────────────────────────────────

#[test]
fn frame_iter_with_options_pixel_format() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let config = FrameOutputOptions {
        pixel_format: PixelFormat::Gray8,
        ..FrameOutputOptions::default()
    };

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let iter = unbundler
        .video()
        .frame_iter_with_options(FrameRange::Range(0, 0), config)
        .expect("Failed to create iterator");

    let results: Vec<_> = iter.collect();
    assert_eq!(results.len(), 1);
    let (_, image) = results[0].as_ref().expect("Decode error");
    assert!(
        matches!(image, image::DynamicImage::ImageLuma8(_)),
        "Expected grayscale image",
    );
}

// ── matches frames() output ────────────────────────────────────────

#[test]
fn frame_iter_matches_frames_count() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let range = FrameRange::Range(0, 9);

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let collected = unbundler
        .video()
        .frames(range.clone())
        .expect("Failed to extract");

    let mut unbundler2 = MediaFile::open(path).expect("Failed to open fixture");
    let iter = unbundler2
        .video()
        .frame_iter(range)
        .expect("Failed to create iterator");
    let iter_count = iter.filter(|r| r.is_ok()).count();

    assert_eq!(
        collected.len(), iter_count,
        "frame_iter and frames() should produce same count",
    );
}

// ── error cases ────────────────────────────────────────────────────

#[test]
fn frame_iter_no_video_stream() {
    let path = sample_audio_only_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let result = unbundler.video().frame_iter(FrameRange::Range(0, 0));
    assert!(result.is_err(), "Expected error for audio-only file");

    let error = format!("{}", result.err().unwrap());
    assert!(
        error.contains("video") || error.contains("Video"),
        "Error should mention video stream: {error}",
    );
}

#[test]
fn frame_iter_video_only_file() {
    let path = sample_video_only_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let iter = unbundler
        .video()
        .frame_iter(FrameRange::Range(0, 2))
        .expect("Failed to create iterator");

    let results: Vec<_> = iter.collect();
    assert!(!results.is_empty(), "Should decode frames from video-only file");
}
