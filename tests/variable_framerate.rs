//! VFR analysis integration tests.

use std::path::Path;

use unbundle::MediaFile;

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn analyze_variable_framerate_on_cfr_video() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let analysis = unbundler
        .video()
        .analyze_variable_framerate()
        .expect("vfr analysis");

    // The test fixture is 30 fps constant.
    assert!(
        !analysis.is_variable_frame_rate,
        "expected constant frame rate for test fixture"
    );
    assert!(
        analysis.frames_analyzed > 0,
        "should have analyzed some frames"
    );
    assert!(
        (analysis.mean_frames_per_second - 30.0).abs() < 2.0,
        "expected ~30 fps, got {}",
        analysis.mean_frames_per_second
    );
}

#[test]
fn vfr_analysis_field_consistency() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let analysis = unbundler
        .video()
        .analyze_variable_framerate()
        .expect("vfr analysis");

    assert!(analysis.min_frames_per_second <= analysis.mean_frames_per_second);
    assert!(analysis.mean_frames_per_second <= analysis.max_frames_per_second);
    assert!(analysis.mean_frame_duration > 0.0);
    assert!(analysis.frame_duration_stddev >= 0.0);
}
