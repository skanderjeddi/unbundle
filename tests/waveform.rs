//! Waveform generation integration tests.
//!
//! Requires the `waveform` feature and test fixtures.

#![cfg(feature = "waveform")]

use std::path::Path;

use unbundle::{MediaFile, WaveformOptions};

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn generate_waveform_default() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let config = WaveformOptions::default();
    let waveform = unbundler
        .audio()
        .generate_waveform(&config)
        .expect("waveform");

    assert_eq!(waveform.bins.len(), config.bins);
    assert!(waveform.duration.as_secs_f64() > 0.0);
    assert!(waveform.sample_rate > 0);
    assert!(waveform.total_samples > 0);
}

#[test]
fn waveform_custom_bins() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let config = WaveformOptions {
        bins: 50,
        ..Default::default()
    };
    let waveform = unbundler
        .audio()
        .generate_waveform(&config)
        .expect("waveform");

    assert_eq!(waveform.bins.len(), 50);
}

#[test]
fn waveform_bins_in_range() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let waveform = unbundler
        .audio()
        .generate_waveform(&WaveformOptions::default())
        .expect("waveform");

    for bin in &waveform.bins {
        assert!(
            bin.min >= -1.0 && bin.min <= 1.0,
            "min out of range: {}",
            bin.min
        );
        assert!(
            bin.max >= -1.0 && bin.max <= 1.0,
            "max out of range: {}",
            bin.max
        );
        assert!(
            bin.rms >= 0.0 && bin.rms <= 1.0,
            "rms out of range: {}",
            bin.rms
        );
        assert!(bin.min <= bin.max, "min should be <= max");
    }
}

#[test]
fn waveform_with_aliases_builds() {
    let config = WaveformOptions::new()
        .with_bins(128)
        .with_start(std::time::Duration::from_secs(1))
        .with_end(std::time::Duration::from_secs(3));

    assert_eq!(config.bins, 128);
    assert_eq!(config.start, Some(std::time::Duration::from_secs(1)));
    assert_eq!(config.end, Some(std::time::Duration::from_secs(3)));
}
