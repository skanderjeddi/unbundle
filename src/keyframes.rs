//! Keyframe and GOP (Group of Pictures) analysis.
//!
//! This module provides [`KeyframeInfo`] and [`GopInfo`] for inspecting the
//! keyframe distribution and GOP structure of a video stream without
//! full-frame decoding.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::MediaUnbundler;
//!
//! let mut unbundler = MediaUnbundler::open("input.mp4")?;
//! let gop = unbundler.video().analyze_gops()?;
//! println!("Total keyframes: {}", gop.keyframes.len());
//! println!("Avg GOP size:    {:.1}", gop.average_gop_size);
//! println!("Max GOP size:    {}", gop.max_gop_size);
//! # Ok::<(), unbundle::UnbundleError>(())
//! ```

use std::time::Duration;

use ffmpeg_next::{Error as FfmpegError, Packet, Rational};

use crate::error::UnbundleError;
use crate::unbundler::MediaUnbundler;

/// Information about a single keyframe (sync point).
#[derive(Debug, Clone)]
pub struct KeyframeInfo {
    /// The packet number (0-indexed) of this keyframe among video packets.
    pub packet_number: u64,
    /// Presentation timestamp (if available).
    pub pts: Option<i64>,
    /// Presentation timestamp as a [`Duration`].
    pub timestamp: Option<Duration>,
    /// Packet size in bytes.
    pub size: usize,
}

/// Summary of the GOP (Group of Pictures) structure.
#[derive(Debug, Clone)]
pub struct GopInfo {
    /// All detected keyframes in display order.
    pub keyframes: Vec<KeyframeInfo>,
    /// The sizes (in packets) of each GOP. The i-th entry is the number of
    /// video packets between keyframe i and keyframe i+1 (or the end of
    /// the stream for the last GOP).
    pub gop_sizes: Vec<u64>,
    /// Average GOP size in packets.
    pub average_gop_size: f64,
    /// Minimum GOP size observed.
    pub min_gop_size: u64,
    /// Maximum GOP size observed.
    pub max_gop_size: u64,
    /// Total number of video packets scanned.
    pub total_video_packets: u64,
}

/// Scan the video stream for keyframes and compute GOP statistics.
///
/// This function reads packets without decoding, so it is very fast.
pub(crate) fn analyze_gops_impl(
    unbundler: &mut MediaUnbundler,
    video_stream_index: usize,
) -> Result<GopInfo, UnbundleError> {
    log::debug!("Analyzing GOP structure (stream={})", video_stream_index);
    let time_base: Rational = unbundler
        .input_context
        .stream(video_stream_index)
        .ok_or(UnbundleError::NoVideoStream)?
        .time_base();

    let mut keyframes: Vec<KeyframeInfo> = Vec::new();
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
                        let secs = p as f64
                            * time_base.numerator() as f64
                            / time_base.denominator().max(1) as f64;
                        Duration::from_secs_f64(secs.max(0.0))
                    });

                    keyframes.push(KeyframeInfo {
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

    // Compute GOP sizes.
    let mut gop_sizes: Vec<u64> = Vec::new();
    for i in 0..keyframes.len() {
        let start = keyframes[i].packet_number;
        let end = if i + 1 < keyframes.len() {
            keyframes[i + 1].packet_number
        } else {
            video_packet_count
        };
        gop_sizes.push(end - start);
    }

    let average_gop_size = if gop_sizes.is_empty() {
        0.0
    } else {
        gop_sizes.iter().sum::<u64>() as f64 / gop_sizes.len() as f64
    };
    let min_gop_size = gop_sizes.iter().copied().min().unwrap_or(0);
    let max_gop_size = gop_sizes.iter().copied().max().unwrap_or(0);

    Ok(GopInfo {
        keyframes,
        gop_sizes,
        average_gop_size,
        min_gop_size,
        max_gop_size,
        total_video_packets: video_packet_count,
    })
}
