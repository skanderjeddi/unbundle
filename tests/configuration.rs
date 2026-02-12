//! ExtractOptions, FrameOutputOptions, and PixelFormat tests.
//!
//! Tests require fixture files from `tests/fixtures/generate_fixtures.sh`.

use std::path::Path;
use std::sync::Arc;

use unbundle::{
    ExtractOptions, FrameRange, MediaFile, PixelFormat, ProgressCallback, ProgressInfo,
};

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

// ── ExtractOptions builder ───────────────────────────────────────

#[test]
fn config_defaults() {
    let config = ExtractOptions::new();
    let debug = format!("{config:?}");
    assert!(debug.contains("ExtractOptions"));
    assert!(debug.contains("has_cancellation: false"));
    assert!(debug.contains("batch_size: 1"));
}

#[test]
fn config_with_batch_size() {
    let config = ExtractOptions::new().with_batch_size(10);
    let debug = format!("{config:?}");
    assert!(debug.contains("batch_size: 10"));
}

#[test]
fn config_with_batch_size_clamps_zero() {
    let config = ExtractOptions::new().with_batch_size(0);
    let debug = format!("{config:?}");
    // Clamped to 1.
    assert!(debug.contains("batch_size: 1"));
}

// ── PixelFormat ──────────────────────────────────────────────

#[test]
fn frames_rgb8_default() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let config = ExtractOptions::new().with_pixel_format(PixelFormat::Rgb8);
    let frames = unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 0), &config)
        .expect("Failed to extract");

    assert_eq!(frames.len(), 1);
    // DynamicImage::ImageRgb8
    assert!(
        matches!(frames[0], image::DynamicImage::ImageRgb8(_)),
        "Expected RGB8 image",
    );
}

#[test]
fn frames_rgba8() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let config = ExtractOptions::new().with_pixel_format(PixelFormat::Rgba8);
    let frames = unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 0), &config)
        .expect("Failed to extract");

    assert_eq!(frames.len(), 1);
    assert!(
        matches!(frames[0], image::DynamicImage::ImageRgba8(_)),
        "Expected RGBA8 image",
    );
}

#[test]
fn frames_gray8() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let config = ExtractOptions::new().with_pixel_format(PixelFormat::Gray8);
    let frames = unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 0), &config)
        .expect("Failed to extract");

    assert_eq!(frames.len(), 1);
    assert!(
        matches!(frames[0], image::DynamicImage::ImageLuma8(_)),
        "Expected Luma8 (grayscale) image",
    );
}

// ── Resolution scaling ─────────────────────────────────────────────

#[test]
fn frames_custom_width() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let config = ExtractOptions::new().with_resolution(Some(320), None); // Auto height
    let frames = unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 0), &config)
        .expect("Failed to extract");

    assert_eq!(frames[0].width(), 320);
    // Aspect ratio maintained → height should be 240 (from 640×480).
    assert_eq!(frames[0].height(), 240);
}

#[test]
fn frames_custom_height() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let config = ExtractOptions::new().with_resolution(None, Some(240));
    let frames = unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 0), &config)
        .expect("Failed to extract");

    assert_eq!(frames[0].height(), 240);
    assert_eq!(frames[0].width(), 320);
}

#[test]
fn frames_fixed_resolution() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let config = ExtractOptions::new()
        .with_resolution(Some(200), Some(100))
        .with_maintain_aspect_ratio(false);
    let frames = unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 0), &config)
        .expect("Failed to extract");

    assert_eq!(frames[0].width(), 200);
    assert_eq!(frames[0].height(), 100);
}

// ── FrameOutputOptions resolve_dimensions ───────────────────────────

#[test]
fn frame_output_config_defaults() {
    let config = unbundle::FrameOutputOptions::default();
    assert_eq!(config.pixel_format, PixelFormat::Rgb8);
    assert!(config.width.is_none());
    assert!(config.height.is_none());
    assert!(config.maintain_aspect_ratio);
}

// ── Progress callback fires ────────────────────────────────────────

struct CountingProgress {
    count: std::sync::Mutex<u64>,
}

impl ProgressCallback for CountingProgress {
    fn on_progress(&self, _info: &ProgressInfo) {
        let mut c = self.count.lock().unwrap();
        *c += 1;
    }
}

#[test]
fn progress_callback_fires() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let counter = Arc::new(CountingProgress {
        count: std::sync::Mutex::new(0),
    });

    let mut unbundler = MediaFile::open(path).expect("Failed to open fixture");
    let config = ExtractOptions::new()
        .with_progress(counter.clone())
        .with_batch_size(1);

    unbundler
        .video()
        .frames_with_options(FrameRange::Range(0, 4), &config)
        .expect("Failed to extract");

    let count = *counter.count.lock().unwrap();
    assert!(
        count > 0,
        "Progress callback should have been called at least once"
    );
}
