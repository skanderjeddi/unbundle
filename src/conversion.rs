//! Internal utility functions.
//!
//! Helpers for pixel-data copying, timestamp conversion, and other shared
//! logic that does not belong in any single public module.

use std::time::Duration;

use ffmpeg_next::{Rational, frame::Video as VideoFrame};

/// Copy pixel data from an FFmpeg video frame into a tightly-packed buffer.
///
/// `bytes_per_pixel` is the number of bytes per pixel for the output format
/// (e.g. 3 for RGB24, 4 for RGBA, 1 for GRAY8).
pub fn frame_to_buffer(
    video_frame: &VideoFrame,
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
) -> Vec<u8> {
    let stride = video_frame.stride(0);
    let expected_stride = (width as usize) * bytes_per_pixel;
    let data = video_frame.data(0);

    if stride == expected_stride {
        data[..expected_stride * (height as usize)].to_vec()
    } else {
        let mut buffer = Vec::with_capacity(expected_stride * (height as usize));
        for row in 0..(height as usize) {
            let row_start = row * stride;
            buffer.extend_from_slice(&data[row_start..row_start + expected_stride]);
        }
        buffer
    }
}

/// Convert a [`Duration`] to a timestamp in the stream's time base.
///
/// The result is suitable for passing to FFmpeg seeking functions.
pub fn duration_to_stream_timestamp(duration: Duration, time_base: Rational) -> i64 {
    let seconds = duration.as_secs_f64();
    let numerator = time_base.numerator() as f64;
    let denominator = time_base.denominator() as f64;
    (seconds * denominator / numerator) as i64
}

/// Convert a [`Duration`] to a frame number using the video's frame rate.
pub fn timestamp_to_frame_number(timestamp: Duration, frames_per_second: f64) -> u64 {
    (timestamp.as_secs_f64() * frames_per_second) as u64
}

/// Rescale a PTS value from stream time base to seconds.
pub fn pts_to_seconds(pts: i64, time_base: Rational) -> f64 {
    pts as f64 * time_base.numerator() as f64 / time_base.denominator() as f64
}

/// Rescale a PTS value to a frame number.
pub fn pts_to_frame_number(pts: i64, time_base: Rational, frames_per_second: f64) -> u64 {
    let seconds = pts_to_seconds(pts, time_base);
    (seconds * frames_per_second) as u64
}

/// Convert a frame number to a seek timestamp in AV_TIME_BASE (microseconds).
///
/// `input_context.seek()` (via `avformat_seek_file` with `stream_index = -1`)
/// expects timestamps in AV_TIME_BASE (1/1_000_000). This helper computes the
/// frame's time in seconds and converts directly to microseconds, bypassing
/// the stream time base entirely.
pub fn frame_number_to_seek_timestamp(frame_number: u64, frames_per_second: f64) -> i64 {
    let seconds = frame_number as f64 / frames_per_second;
    (seconds * 1_000_000.0) as i64
}

/// Convert a [`Duration`] to a seek timestamp in AV_TIME_BASE (microseconds).
///
/// `input_context.seek()` (via `avformat_seek_file` with `stream_index = -1`)
/// expects timestamps in AV_TIME_BASE (1/1_000_000). This is the correct
/// conversion for container-level seeking.
pub fn duration_to_seek_timestamp(duration: Duration) -> i64 {
    duration.as_micros() as i64
}
