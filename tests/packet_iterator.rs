//! Packet iterator integration tests.

use std::path::Path;

use unbundle::MediaFile;

fn sample_video_path() -> &'static str {
    "tests/fixtures/sample_video.mp4"
}

#[test]
fn packet_iterator_yields_packets() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let packets: Vec<_> = unbundler
        .packet_iter()
        .expect("packet_iter")
        .filter_map(|r| r.ok())
        .collect();

    assert!(!packets.is_empty(), "expected at least one packet");

    // All packets should have positive sizes.
    for pkt in &packets {
        assert!(pkt.size > 0, "packet size should be positive");
    }
}

#[test]
fn packet_iterator_has_keyframes() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let has_keyframe = unbundler
        .packet_iter()
        .expect("packet_iter")
        .filter_map(|r| r.ok())
        .any(|p| p.is_keyframe);

    assert!(has_keyframe, "expected at least one keyframe packet");
}

#[test]
fn packet_iterator_has_consistent_stream_indices() {
    let path = sample_video_path();
    if !Path::new(path).exists() {
        return;
    }

    let mut unbundler = MediaFile::open(path).expect("open");
    let stream_count = {
        let meta = unbundler.metadata();
        let mut count = 0usize;
        if meta.video.is_some() {
            count += 1;
        }
        if meta.audio.is_some() {
            count += 1;
        }
        count
    };

    for pkt in unbundler
        .packet_iter()
        .expect("packet_iter")
        .filter_map(|r| r.ok())
    {
        assert!(
            pkt.stream_index < stream_count,
            "stream_index {} out of range (count {})",
            pkt.stream_index,
            stream_count
        );
    }
}
