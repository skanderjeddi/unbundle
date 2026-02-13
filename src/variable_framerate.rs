//! Variable frame rate (VFR) detection and analysis.
//!
//! This module provides [`VariableFrameRateAnalysis`] for detecting whether a video stream
//! uses a constant or variable frame rate, and computing per-frame timing
//! statistics.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{MediaFile, UnbundleError};
//!
//! let mut unbundler = MediaFile::open("input.mp4")?;
//! let analysis = unbundler.video().analyze_variable_framerate()?;
//! if analysis.is_variable_frame_rate {
//!     println!("VFR detected! FPS range: {:.2}\u{2013}{:.2}",
//!         analysis.min_frames_per_second, analysis.max_frames_per_second);
//! }
//! # Ok::<(), UnbundleError>(())
//! ```

use std::time::Duration;

use ffmpeg_next::{Error as FfmpegError, Packet, Rational};

use crate::error::UnbundleError;
use crate::unbundle::MediaFile;

/// Results of VFR analysis on a video stream.
#[derive(Debug, Clone)]
pub struct VariableFrameRateAnalysis {
    /// Whether the stream appears to be variable frame rate.
    ///
    /// This is `true` when the standard deviation of frame durations exceeds
    /// 10% of the mean frame duration.
    pub is_variable_frame_rate: bool,
    /// Mean frame duration in seconds.
    pub mean_frame_duration: f64,
    /// Standard deviation of frame durations in seconds.
    pub frame_duration_stddev: f64,
    /// Minimum instantaneous FPS observed.
    pub min_frames_per_second: f64,
    /// Maximum instantaneous FPS observed.
    pub max_frames_per_second: f64,
    /// Mean FPS (1 / mean_frame_duration).
    pub mean_frames_per_second: f64,
    /// Number of frames analyzed.
    pub frames_analyzed: u64,
    /// Per-frame PTS values converted to [`Duration`], in decode order.
    pub pts_list: Vec<Duration>,
}

/// Analyze the PTS distribution of a video stream to detect VFR.
///
/// Reads all video-stream packets and collects their PTS values.
pub(crate) fn analyze_variable_framerate_impl(
    unbundler: &mut MediaFile,
    video_stream_index: usize,
) -> Result<VariableFrameRateAnalysis, UnbundleError> {
    log::debug!("Analyzing VFR (stream={})", video_stream_index);
    let time_base: Rational = unbundler
        .input_context
        .stream(video_stream_index)
        .ok_or(UnbundleError::NoVideoStream)?
        .time_base();

    let time_base_numerator = time_base.numerator() as f64;
    let time_base_denominator = time_base.denominator().max(1) as f64;

    let mut pts_values: Vec<i64> = Vec::new();
    let mut packet = Packet::empty();
    loop {
        match packet.read(&mut unbundler.input_context) {
            Ok(()) => {
                if packet.stream() as usize != video_stream_index {
                    continue;
                }
                if let Some(pts) = packet.pts() {
                    pts_values.push(pts);
                }
            }
            Err(FfmpegError::Eof) => break,
            Err(e) => return Err(UnbundleError::from(e)),
        }
    }

    // Sort PTS values (display order).
    pts_values.sort_unstable();

    let pts_list: Vec<Duration> = pts_values
        .iter()
        .map(|&p| {
            let secs = p as f64 * time_base_numerator / time_base_denominator;
            Duration::from_secs_f64(secs.max(0.0))
        })
        .collect();

    if pts_values.len() < 2 {
        return Ok(VariableFrameRateAnalysis {
            is_variable_frame_rate: false,
            mean_frame_duration: 0.0,
            frame_duration_stddev: 0.0,
            min_frames_per_second: 0.0,
            max_frames_per_second: 0.0,
            mean_frames_per_second: 0.0,
            frames_analyzed: pts_values.len() as u64,
            pts_list,
        });
    }

    // Compute frame durations (in seconds).
    let durations: Vec<f64> = pts_values
        .windows(2)
        .map(|w| ((w[1] - w[0]) as f64) * time_base_numerator / time_base_denominator)
        .filter(|&d| d > 0.0)
        .collect();

    if durations.is_empty() {
        return Ok(VariableFrameRateAnalysis {
            is_variable_frame_rate: false,
            mean_frame_duration: 0.0,
            frame_duration_stddev: 0.0,
            min_frames_per_second: 0.0,
            max_frames_per_second: 0.0,
            mean_frames_per_second: 0.0,
            frames_analyzed: pts_values.len() as u64,
            pts_list,
        });
    }

    let mean = durations.iter().sum::<f64>() / durations.len() as f64;
    let variance =
        durations.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / durations.len() as f64;
    let stddev = variance.sqrt();

    let min_duration = durations.iter().copied().fold(f64::INFINITY, f64::min);
    let max_duration = durations.iter().copied().fold(0.0_f64, f64::max);

    let max_frames_per_second = if min_duration > 0.0 {
        1.0 / min_duration
    } else {
        0.0
    };
    let min_frames_per_second = if max_duration > 0.0 {
        1.0 / max_duration
    } else {
        0.0
    };
    let mean_frames_per_second = if mean > 0.0 { 1.0 / mean } else { 0.0 };

    // Clamp to the observed range to avoid floating-point rounding artifacts
    // where 1/mean lands slightly outside [min_frames_per_second, max_frames_per_second].
    let mean_frames_per_second =
        mean_frames_per_second.clamp(min_frames_per_second, max_frames_per_second);

    // VFR if stddev > 10% of mean frame duration.
    let is_variable_frame_rate = mean > 0.0 && (stddev / mean) > 0.10;

    Ok(VariableFrameRateAnalysis {
        is_variable_frame_rate,
        mean_frame_duration: mean,
        frame_duration_stddev: stddev,
        min_frames_per_second,
        max_frames_per_second,
        mean_frames_per_second,
        frames_analyzed: pts_values.len() as u64,
        pts_list,
    })
}
