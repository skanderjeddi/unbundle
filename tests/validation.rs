//! Validation integration tests.
//!
//! Tests require fixture files from `tests/fixtures/generate_fixtures.sh`.

use std::path::Path;

use unbundle::MediaFile;

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

fn sample_audio_only_path() -> &'static str {
    "tests/fixtures/sample_audio_only.mp4"
}

fn sample_video_only_path() -> &'static str {
    "tests/fixtures/sample_video_only.mp4"
}

#[test]
fn validate_normal_video() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let report = unbundler.validate();

    assert!(report.is_valid(), "Normal video should be valid");
    assert!(report.errors.is_empty(), "No errors expected");
}

#[test]
fn validate_has_info() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let report = unbundler.validate();

    assert!(!report.info.is_empty(), "Expected info entries");
    // Should describe video stream.
    let has_video_info = report.info.iter().any(|s| s.contains("Video:"));
    assert!(has_video_info, "Info should include video description");
    // Should describe audio stream.
    let has_audio_info = report.info.iter().any(|s| s.contains("Audio:"));
    assert!(has_audio_info, "Info should include audio description");
}

#[test]
fn validate_audio_only() {
    let path = sample_audio_only_path();
    if !Path::new(path).exists() {
        return;
    }

    let unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let report = unbundler.validate();

    assert!(report.is_valid(), "Audio-only file should still be valid");
    let has_no_video = report.info.iter().any(|s| s.contains("No video"));
    assert!(has_no_video, "Should note missing video stream");
}

#[test]
fn validate_video_only() {
    let path = sample_video_only_path();
    if !Path::new(path).exists() {
        return;
    }

    let unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let report = unbundler.validate();

    assert!(report.is_valid(), "Video-only file should still be valid");
    let has_no_audio = report.info.iter().any(|s| s.contains("No audio"));
    assert!(has_no_audio, "Should note missing audio stream");
}

#[test]
fn validate_display_impl() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let report = unbundler.validate();
    let display = format!("{report}");

    assert!(!display.is_empty(), "Display output should not be empty");
    assert!(display.contains("[INFO]"), "Display should include [INFO] labels");
}

#[test]
fn validate_issue_count() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let report = unbundler.validate();

    let expected = report.info.len() + report.warnings.len() + report.errors.len();
    assert_eq!(report.issue_count(), expected);
}

#[test]
fn validate_is_valid_when_no_errors() {
    let report = unbundle::ValidationReport {
        info: vec!["some info".to_string()],
        warnings: vec!["some warning".to_string()],
        errors: vec![],
    };
    assert!(report.is_valid());

    let bad_report = unbundle::ValidationReport {
        info: vec![],
        warnings: vec![],
        errors: vec!["fatal problem".to_string()],
    };
    assert!(!bad_report.is_valid());
}
