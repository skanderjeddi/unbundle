//! Hardware acceleration integration tests (feature = "hw-accel").
//!
//! These tests verify the HW device enumeration API. Actual hardware
//! decoding cannot be reliably tested in CI because available devices
//! depend on the host GPU and driver stack.

#![cfg(feature = "hw-accel")]

use std::path::Path;

use unbundle::{ExtractionConfig, FrameRange, HwAccelMode, MediaUnbundler};

const SAMPLE_VIDEO: &str = "tests/fixtures/sample_video.mp4";

fn skip_unless(path: &str) -> bool {
    if !Path::new(path).exists() {
        eprintln!("Skipping: fixture {path} not found");
        return true;
    }
    false
}

#[test]
fn enumerate_hw_devices_does_not_panic() {
    let devices = unbundle::hw_accel::available_hw_devices();
    println!("Detected HW devices: {devices:?}");

    // Sanity: every returned device should round-trip through its Debug repr.
    for device in &devices {
        let _repr = format!("{device:?}");
    }
}

#[test]
fn hw_accel_auto_mode_extracts_frames() {
    if skip_unless(SAMPLE_VIDEO) {
        return;
    }

    let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
    let config = ExtractionConfig::new().with_hw_accel(HwAccelMode::Auto);

    let frames = unbundler
        .video()
        .frames_with_config(FrameRange::Specific(vec![0, 1, 2]), &config)
        .unwrap();

    assert_eq!(frames.len(), 3, "Auto mode should still extract frames");
}

#[test]
fn hw_accel_software_fallback() {
    if skip_unless(SAMPLE_VIDEO) {
        return;
    }

    let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
    let config = ExtractionConfig::new().with_hw_accel(HwAccelMode::Software);

    let frame = unbundler
        .video()
        .frames_with_config(FrameRange::Specific(vec![0]), &config)
        .unwrap();

    assert_eq!(frame.len(), 1);
    assert!(frame[0].width() > 0);
}
