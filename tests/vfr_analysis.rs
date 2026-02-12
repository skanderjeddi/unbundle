//! VFR analysis integration tests.

use std::path::Path;

use unbundle::MediaUnbundler;

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn analyze_vfr_on_cfr_video() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let analysis = unbundler.video().analyze_vfr().expect("vfr analysis");

    // The test fixture is 30 fps constant.
    assert!(
        !analysis.is_vfr,
        "expected constant frame rate for test fixture"
    );
    assert!(
        analysis.frames_analyzed > 0,
        "should have analyzed some frames"
    );
    assert!(
        (analysis.mean_fps - 30.0).abs() < 2.0,
        "expected ~30 fps, got {}",
        analysis.mean_fps
    );
}

#[test]
fn vfr_analysis_field_consistency() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let analysis = unbundler.video().analyze_vfr().expect("vfr analysis");

    assert!(analysis.min_fps <= analysis.mean_fps);
    assert!(analysis.mean_fps <= analysis.max_fps);
    assert!(analysis.mean_frame_duration > 0.0);
    assert!(analysis.frame_duration_stddev >= 0.0);
}
