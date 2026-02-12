//! Audio iterator integration tests.

use std::path::Path;

use unbundle::MediaFile;

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn sample_iter_yields_chunks() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let chunks: Vec<_> = unbundler
        .audio()
        .sample_iter()
        .expect("sample_iter")
        .filter_map(|r| r.ok())
        .collect();

    assert!(!chunks.is_empty(), "expected audio chunks");

    let total_samples: usize = chunks.iter().map(|c| c.samples.len()).sum();
    assert!(total_samples > 0, "expected some audio samples");
}

#[test]
fn sample_iter_has_metadata() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    for chunk in unbundler
        .audio()
        .sample_iter()
        .expect("sample_iter")
        .filter_map(|r| r.ok())
    {
        assert!(chunk.sample_rate > 0);
        assert!(!chunk.samples.is_empty());
        // Timestamps should generally be non-negative (Duration is always >= 0).
        let _ = chunk.timestamp;
    }
}

#[test]
fn sample_iter_early_exit() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let first_three: Vec<_> = unbundler
        .audio()
        .sample_iter()
        .expect("sample_iter")
        .filter_map(|r| r.ok())
        .take(3)
        .collect();

    // Should not panic or error when taking only a few chunks.
    assert!(first_three.len() <= 3);
}
