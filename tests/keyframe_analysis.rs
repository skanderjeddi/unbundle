//! Keyframe and GOP analysis integration tests.

use std::path::Path;

use unbundle::MediaUnbundler;

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn keyframes_returns_list() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let keyframes = unbundler.video().keyframes().expect("keyframes");

    assert!(!keyframes.is_empty(), "expected at least one keyframe");

    // First keyframe should be near the start.
    assert_eq!(
        keyframes[0].packet_number, 0,
        "first keyframe should be packet 0"
    );
}

#[test]
fn gop_analysis() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let gop_info = unbundler.video().analyze_gops().expect("gop analysis");

    assert!(
        gop_info.total_video_packets > 0,
        "expected at least some video packets"
    );
    assert!(
        !gop_info.keyframes.is_empty(),
        "expected at least one keyframe in GOP info"
    );
    assert!(
        gop_info.average_gop_size > 0.0,
        "average GOP size should be positive"
    );
    assert!(
        gop_info.min_gop_size > 0,
        "min GOP size should be positive"
    );
    assert!(
        gop_info.max_gop_size >= gop_info.min_gop_size,
        "max >= min"
    );
}

#[test]
fn keyframes_have_timestamps() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaUnbundler::open(path).expect("open");
    let keyframes = unbundler.video().keyframes().expect("keyframes");

    for kf in &keyframes {
        // Timestamps should be non-negative when present.
        if let Some(ts) = kf.timestamp {
            assert!(
                ts.as_secs_f64() >= 0.0,
                "keyframe timestamps should be non-negative"
            );
        }
    }
}
