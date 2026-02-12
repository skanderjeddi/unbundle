//! Hardware acceleration integration tests (feature = "hardware").
//!
//! These tests verify the HW device enumeration API. Actual hardware
//! decoding cannot be reliably tested in CI because available devices
//! depend on the host GPU and driver stack.

#![cfg(feature = "hardware")]

use std::path::Path;

use unbundle::{ExtractOptions, FrameRange, HardwareAccelerationMode, MediaFile};

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
    let devices = unbundle::hardware_acceleration::available_hardware_devices();
    println!("Detected HW devices: {devices:?}");

    // Sanity: every returned device should round-trip through its Debug repr.
    for device in &devices {
        let _repr = format!("{device:?}");
    }
}

#[test]
fn hardware_acceleration_auto_mode_extracts_frames() {
    if skip_unless(SAMPLE_VIDEO) {
        return;
    }

    let mut unbundler = MediaFile::open(SAMPLE_VIDEO).unwrap();
    let config = ExtractOptions::new().with_hardware_acceleration(HardwareAccelerationMode::Auto);

    let frames = unbundler
        .video()
        .frames_with_options(FrameRange::Specific(vec![0, 1, 2]), &config)
        .unwrap();

    assert_eq!(frames.len(), 3, "Auto mode should still extract frames");
}

#[test]
fn hardware_acceleration_software_fallback() {
    if skip_unless(SAMPLE_VIDEO) {
        return;
    }

    let mut unbundler = MediaFile::open(SAMPLE_VIDEO).unwrap();
    let config =
        ExtractOptions::new().with_hardware_acceleration(HardwareAccelerationMode::Software);

    let frame = unbundler
        .video()
        .frames_with_options(FrameRange::Specific(vec![0]), &config)
        .unwrap();

    assert_eq!(frame.len(), 1);
    assert!(frame[0].width() > 0);
}
