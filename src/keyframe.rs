//! Keyframe and Group of Pictures analysis.
//!
//! This module provides [`KeyFrameMetadata`] and [`GroupOfPicturesInfo`] for inspecting the
//! keyframe distribution and Group of Pictures structure of a video stream without
//! full-frame decoding.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{MediaFile, UnbundleError};
//!
//! let mut unbundler = MediaFile::open("input.mp4")?;
//! let group_of_pictures = unbundler.video().analyze_group_of_pictures()?;
//! println!("Total keyframes: {}", group_of_pictures.keyframes.len());
//! println!("Average Group of Pictures size: {:.1}", group_of_pictures.average_group_of_pictures_size);
//! println!("Max Group of Pictures size: {}", group_of_pictures.max_group_of_pictures_size);
//! # Ok::<(), UnbundleError>(())
//! ```

use std::time::Duration;

use ffmpeg_next::{Error as FfmpegError, Packet, Rational};

use crate::error::UnbundleError;
use crate::unbundle::MediaFile;

/// Information about a single keyframe (sync point).
#[derive(Debug, Clone)]
pub struct KeyFrameMetadata {
    /// The packet number (0-indexed) of this keyframe among video packets.
    pub packet_number: u64,
    /// Presentation timestamp (if available).
    pub pts: Option<i64>,
    /// Presentation timestamp as a [`Duration`].
    pub timestamp: Option<Duration>,
    /// Packet size in bytes.
    pub size: usize,
}

/// Summary of the Group of Pictures structure.
#[derive(Debug, Clone)]
pub struct GroupOfPicturesInfo {
    /// All detected keyframes in display order.
    pub keyframes: Vec<KeyFrameMetadata>,
    /// The sizes (in packets) of each Group of Pictures sequence. The i-th entry is the number of
    /// video packets between keyframe i and keyframe i+1 (or the end of
    /// the stream for the last sequence.
    pub group_of_pictures_sizes: Vec<u64>,
    /// Average Group of Pictures size in packets.
    pub average_group_of_pictures_size: f64,
    /// Minimum Group of Pictures size observed.
    pub min_group_of_pictures_size: u64,
    /// Maximum Group of Pictures size observed.
    pub max_group_of_pictures_size: u64,
    /// Total number of video packets scanned.
    pub total_video_packets: u64,
}

/// Scan the video stream for keyframes and compute Group of Pictures statistics.
///
/// This function reads packets without decoding, so it is very fast.
pub(crate) fn analyze_group_of_pictures_impl(
    unbundler: &mut MediaFile,
    video_stream_index: usize,
) -> Result<GroupOfPicturesInfo, UnbundleError> {
    log::debug!(
        "Analyzing Group of Pictures structure (stream={})",
        video_stream_index
    );
    let time_base: Rational = unbundler
        .input_context
        .stream(video_stream_index)
        .ok_or(UnbundleError::NoVideoStream)?
        .time_base();

    let mut keyframes: Vec<KeyFrameMetadata> = Vec::new();
    let mut video_packet_count: u64 = 0;

    let mut packet = Packet::empty();
    loop {
        match packet.read(&mut unbundler.input_context) {
            Ok(()) => {
                if packet.stream() as usize != video_stream_index {
                    continue;
                }

                if packet.is_key() {
                    let pts = packet.pts();
                    let timestamp = pts.map(|p| {
                        let secs = p as f64 * time_base.numerator() as f64
                            / time_base.denominator().max(1) as f64;
                        Duration::from_secs_f64(secs.max(0.0))
                    });

                    keyframes.push(KeyFrameMetadata {
                        packet_number: video_packet_count,
                        pts,
                        timestamp,
                        size: packet.size(),
                    });
                }

                video_packet_count += 1;
            }
            Err(FfmpegError::Eof) => break,
            Err(e) => return Err(UnbundleError::from(e)),
        }
    }

    // Compute Group of Pictures sizes.
    let mut group_of_pictures_sizes: Vec<u64> = Vec::new();
    for i in 0..keyframes.len() {
        let start = keyframes[i].packet_number;
        let end = if i + 1 < keyframes.len() {
            keyframes[i + 1].packet_number
        } else {
            video_packet_count
        };
        group_of_pictures_sizes.push(end - start);
    }

    let average_group_of_pictures_size = if group_of_pictures_sizes.is_empty() {
        0.0
    } else {
        group_of_pictures_sizes.iter().sum::<u64>() as f64 / group_of_pictures_sizes.len() as f64
    };
    let min_group_of_pictures_size = group_of_pictures_sizes.iter().copied().min().unwrap_or(0);
    let max_group_of_pictures_size = group_of_pictures_sizes.iter().copied().max().unwrap_or(0);

    Ok(GroupOfPicturesInfo {
        keyframes,
        group_of_pictures_sizes,
        average_group_of_pictures_size,
        min_group_of_pictures_size,
        max_group_of_pictures_size,
        total_video_packets: video_packet_count,
    })
}
