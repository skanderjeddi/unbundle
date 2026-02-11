//! Internal utility functions.
//!
//! Helpers for pixel-data copying, timestamp conversion, and other shared
//! logic that does not belong in any single public module.

use std::time::Duration;

use ffmpeg_next::{Rational, frame::Video as VideoFrame};

/// Copy pixel data from an FFmpeg video frame into a tightly-packed RGB buffer.
///
/// FFmpeg frames frequently carry per-row padding (stride > width × 3).
/// This function strips that padding so the result can be passed directly to
/// [`image::RgbImage::from_raw`].
pub fn frame_to_rgb_buffer(video_frame: &VideoFrame, width: u32, height: u32) -> Vec<u8> {
    let stride = video_frame.stride(0);
    let expected_stride = (width as usize) * 3;
    let data = video_frame.data(0);

    if stride == expected_stride {
        // No padding — fast path: copy the entire plane at once.
        data[..expected_stride * (height as usize)].to_vec()
    } else {
        // Stride includes padding bytes — copy row by row.
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

/// Convert a frame number to a timestamp in the stream's time base.
pub fn frame_number_to_stream_timestamp(
    frame_number: u64,
    frames_per_second: f64,
    time_base: Rational,
) -> i64 {
    let seconds = frame_number as f64 / frames_per_second;
    let duration = Duration::from_secs_f64(seconds);
    duration_to_stream_timestamp(duration, time_base)
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
