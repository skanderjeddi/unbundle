//! Keyframe and Group of Pictures analysis integration tests.

use std::path::Path;

use unbundle::MediaFile;

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn keyframes_returns_list() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let keyframes = unbundler.video().keyframes().expect("keyframes");

    assert!(!keyframes.is_empty(), "expected at least one keyframe");

    // First keyframe should be near the start.
    assert_eq!(
        keyframes[0].packet_number, 0,
        "first keyframe should be packet 0"
    );
}

#[test]
fn group_of_pictures_analysis() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let group_of_pictures_info = unbundler
        .video()
        .analyze_group_of_pictures()
        .expect("group of pictures analysis");

    assert!(
        group_of_pictures_info.total_video_packets > 0,
        "expected at least some video packets"
    );
    assert!(
        !group_of_pictures_info.keyframes.is_empty(),
        "expected at least one keyframe in Group of Pictures info"
    );
    assert!(
        group_of_pictures_info.average_group_of_pictures_size > 0.0,
        "average Group of Pictures size should be positive"
    );
    assert!(
        group_of_pictures_info.min_group_of_pictures_size > 0,
        "minimum Group of Pictures size should be positive"
    );
    assert!(
        group_of_pictures_info.max_group_of_pictures_size
            >= group_of_pictures_info.min_group_of_pictures_size,
        "max >= min"
    );
}

#[test]
fn keyframes_have_timestamps() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
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
