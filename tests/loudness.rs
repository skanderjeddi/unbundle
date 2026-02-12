//! Loudness analysis integration tests.
//!
//! Requires the `loudness` feature and test fixtures.

#![cfg(feature = "loudness")]

use std::path::Path;

use unbundle::MediaFile;

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn analyze_loudness() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let info = unbundler.audio().analyze_loudness().expect("loudness");

    assert!(info.peak > 0.0, "peak should be positive");
    assert!(info.rms > 0.0, "rms should be positive");
    assert!(info.peak_dbfs <= 0.0, "peak dBFS should be <= 0");
    assert!(info.rms_dbfs <= 0.0, "rms dBFS should be <= 0");
    assert!(info.total_samples > 0);
    assert!(info.duration.as_secs_f64() > 0.0);
}

#[test]
fn loudness_peak_ge_rms() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let info = unbundler.audio().analyze_loudness().expect("loudness");

    assert!(
        info.peak >= info.rms,
        "peak ({}) should be >= rms ({})",
        info.peak,
        info.rms
    );
    assert!(
        info.peak_dbfs >= info.rms_dbfs,
        "peak_dbfs ({}) >= rms_dbfs ({})",
        info.peak_dbfs,
        info.rms_dbfs
    );
}

#[test]
fn loudness_on_audio_only() {
    let path = "tests/fixtures/sample_audio_only.mp4";
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let info = unbundler.audio().analyze_loudness().expect("loudness");

    assert!(info.total_samples > 0);
}
