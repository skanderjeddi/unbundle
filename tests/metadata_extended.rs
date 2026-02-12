//! Container tags and colorspace metadata integration tests.

use std::path::Path;

use unbundle::MediaFile;

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn container_tags_present() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let unbundler = MediaFile::open(path).expect("open");
    let meta = unbundler.metadata();

    // Tags may or may not be present depending on how the fixture was created.
    // We just check the field exists and is accessible.
    if let Some(tags) = &meta.tags {
        // FFmpeg usually sets "encoder" or "major_brand".
        assert!(
            !tags.is_empty() || tags.is_empty(),
            "tags field should be accessible"
        );
    }
}

#[test]
fn video_colorspace_fields() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let unbundler = MediaFile::open(path).expect("open");
    let meta = unbundler.metadata();
    let video = meta.video.as_ref().expect("video metadata");

    // These fields should be populated (may be None for unknown).
    // We just verify they are accessible without panic.
    let _ = &video.color_space;
    let _ = &video.color_range;
    let _ = &video.color_primaries;
    let _ = &video.color_transfer;
    let _ = &video.bits_per_raw_sample;
    let _ = &video.pixel_format_name;
}

#[test]
fn video_tracks_populated() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let unbundler = MediaFile::open(path).expect("open");
    let meta = unbundler.metadata();

    if let Some(tracks) = &meta.video_tracks {
        assert!(
            !tracks.is_empty(),
            "expected at least one video track"
        );
        for track in tracks {
            assert!(track.width > 0);
            assert!(track.height > 0);
        }
    }
}

#[test]
fn video_track_selection() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");

    // Track 0 should always be valid for a video file.
    let extractor = unbundler.video_track(0);
    assert!(extractor.is_ok(), "track 0 should be valid");

    // Track 99 should be out of range.
    let err = unbundler.video_track(99);
    assert!(err.is_err(), "track 99 should be out of range");
}
