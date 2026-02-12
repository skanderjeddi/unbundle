//! Waveform generation integration tests.
//!
//! Requires the `waveform` feature and test fixtures.

use std::path::Path;

use unbundle::{MediaUnbundler, WaveformConfig};

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn generate_waveform_default() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let config = WaveformConfig::default();
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

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let config = WaveformConfig { bins: 50, ..Default::default() };
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

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let waveform = unbundler
        .audio()
        .generate_waveform(&WaveformConfig::default())
        .expect("waveform");

    for bin in &waveform.bins {
        assert!(bin.min >= -1.0 && bin.min <= 1.0, "min out of range: {}", bin.min);
        assert!(bin.max >= -1.0 && bin.max <= 1.0, "max out of range: {}", bin.max);
        assert!(bin.rms >= 0.0 && bin.rms <= 1.0, "rms out of range: {}", bin.rms);
        assert!(bin.min <= bin.max, "min should be <= max");
    }
}
