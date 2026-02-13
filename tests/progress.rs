//! Progress and cancellation integration tests.
//!
//! Tests require fixture files from `tests/fixtures/generate_fixtures.sh`.

use std::path::Path;
use std::sync::Arc;

use unbundle::{
    CancellationToken, ExtractOptions, FrameRange, MediaFile, OperationType, ProgressCallback,
    ProgressInfo, UnbundleError,
};

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

fn sample_with_subtitles_path() -> &'static str {
    "tests/fixtures/sample_with_subtitles.mkv"
}

// ── CancellationToken ──────────────────────────────────────────────

#[test]
fn cancellation_token_default_not_cancelled() {
    let token = CancellationToken::new();
    assert!(!token.is_cancelled());
}

#[test]
fn cancellation_token_cancel() {
    let token = CancellationToken::new();
    token.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn cancellation_token_clone_shares_state() {
    let token = CancellationToken::new();
    let clone = token.clone();
    assert!(!clone.is_cancelled());

    token.cancel();
    assert!(clone.is_cancelled());
}

#[test]
fn cancellation_token_default_trait() {
    let token = CancellationToken::default();
    assert!(!token.is_cancelled());
}

#[test]
fn cancelled_extraction_returns_error() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let token = CancellationToken::new();
    token.cancel(); // Cancel immediately.

    let config = ExtractOptions::new().with_cancellation(token);

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let result = unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 99), &config);

    assert!(result.is_err());
    match result.unwrap_err() {
        UnbundleError::Cancelled => {}
        other => panic!("Expected Cancelled, got: {other}"),
    }
}

// ── ProgressInfo ───────────────────────────────────────────────────

struct RecordingProgress {
    infos: std::sync::Mutex<Vec<ProgressInfo>>,
}

impl ProgressCallback for RecordingProgress {
    fn on_progress(&self, info: &ProgressInfo) {
        self.infos.lock().unwrap().push(info.clone());
    }
}

#[test]
fn progress_reports_frame_extraction_operation() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let recorder = Arc::new(RecordingProgress {
        infos: std::sync::Mutex::new(Vec::new()),
    });
    let config = ExtractOptions::new()
        .with_progress(recorder.clone())
        .with_batch_size(1);

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 4), &config)
        .expect("Failed to extract");

    let infos = recorder.infos.lock().unwrap();
    assert!(!infos.is_empty(), "Expected progress callbacks");

    // All should report FrameExtraction operation.
    for info in infos.iter() {
        assert_eq!(info.operation, OperationType::FrameExtraction);
    }
}

#[test]
fn progress_current_increases() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let recorder = Arc::new(RecordingProgress {
        infos: std::sync::Mutex::new(Vec::new()),
    });
    let config = ExtractOptions::new()
        .with_progress(recorder.clone())
        .with_batch_size(1);

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 9), &config)
        .expect("Failed to extract");

    let infos = recorder.infos.lock().unwrap();
    // Verify `current` is monotonically non-decreasing.
    for window in infos.windows(2) {
        assert!(
            window[1].current >= window[0].current,
            "Progress current should be non-decreasing",
        );
    }
}

#[test]
fn progress_has_elapsed() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let recorder = Arc::new(RecordingProgress {
        infos: std::sync::Mutex::new(Vec::new()),
    });
    let config = ExtractOptions::new()
        .with_progress(recorder.clone())
        .with_batch_size(1);

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 2), &config)
        .expect("Failed to extract");

    let infos = recorder.infos.lock().unwrap();
    if let Some(last) = infos.last() {
        // Elapsed should be a reasonable positive value (measured from start).
        assert!(
            last.elapsed.as_nanos() > 0,
            "Expected positive elapsed time",
        );
    }
}

// ── OperationType Debug ────────────────────────────────────────────

#[test]
fn operation_type_debug() {
    let op = OperationType::FrameExtraction;
    let debug = format!("{op:?}");
    assert_eq!(debug, "FrameExtraction");
}

#[test]
fn progress_reports_stream_copy_video_operation() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let recorder = Arc::new(RecordingProgress {
        infos: std::sync::Mutex::new(Vec::new()),
    });
    let config = ExtractOptions::new()
        .with_progress(recorder.clone())
        .with_batch_size(1);

    let output = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let output_path = output.path().with_extension("mp4");

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    unbundler
        .video()
        .stream_copy_with_options(&output_path, &config)
        .expect("Failed to stream-copy video");

    let infos = recorder.infos.lock().unwrap();
    assert!(!infos.is_empty(), "Expected progress callbacks");
    for info in infos.iter() {
        assert_eq!(info.operation, OperationType::StreamCopy);
    }

    let _ = std::fs::remove_file(&output_path);
}

#[test]
fn progress_reports_stream_copy_audio_operation() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let recorder = Arc::new(RecordingProgress {
        infos: std::sync::Mutex::new(Vec::new()),
    });
    let config = ExtractOptions::new()
        .with_progress(recorder.clone())
        .with_batch_size(1);

    let output = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let output_path = output.path().with_extension("aac");

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    unbundler
        .audio()
        .stream_copy_with_options(&output_path, &config)
        .expect("Failed to stream-copy audio");

    let infos = recorder.infos.lock().unwrap();
    assert!(!infos.is_empty(), "Expected progress callbacks");
    for info in infos.iter() {
        assert_eq!(info.operation, OperationType::StreamCopy);
    }

    let _ = std::fs::remove_file(&output_path);
}

#[test]
fn progress_reports_stream_copy_subtitle_operation() {
    let path = sample_with_subtitles_path();
    if !Path::new(path).exists() {
        return;
    }

    let recorder = Arc::new(RecordingProgress {
        infos: std::sync::Mutex::new(Vec::new()),
    });
    let config = ExtractOptions::new()
        .with_progress(recorder.clone())
        .with_batch_size(1);

    let output = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let output_path = output.path().with_extension("mkv");

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    unbundler
        .subtitle()
        .stream_copy_with_options(&output_path, &config)
        .expect("Failed to stream-copy subtitle");

    let infos = recorder.infos.lock().unwrap();
    assert!(!infos.is_empty(), "Expected progress callbacks");
    for info in infos.iter() {
        assert_eq!(info.operation, OperationType::StreamCopy);
    }

    let _ = std::fs::remove_file(&output_path);
}
