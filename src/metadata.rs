//! Media metadata types.
//!
//! This module defines the metadata structures returned by
//! [`MediaUnbundler::metadata`](crate::MediaUnbundler::metadata). Metadata is
//! extracted once when the file is opened and cached for the lifetime of the
//! unbundler.

use std::time::Duration;

/// Complete metadata for a media file.
///
/// Contains optional video and audio stream metadata, plus container-level
/// information such as total duration and format name.
///
/// # Example
///
/// ```no_run
/// use unbundle::MediaUnbundler;
///
/// let unbundler = MediaUnbundler::open("input.mp4").unwrap();
/// let metadata = unbundler.metadata();
/// println!("Duration: {:?}", metadata.duration);
/// println!("Format: {}", metadata.format);
/// ```
#[derive(Debug, Clone)]
#[must_use]
pub struct MediaMetadata {
    /// Video stream metadata, if a video stream is present.
    pub video: Option<VideoMetadata>,
    /// Audio stream metadata, if an audio stream is present.
    pub audio: Option<AudioMetadata>,
    /// Total duration of the media file.
    pub duration: Duration,
    /// Container format name (e.g. `"mp4"`, `"matroska"`, `"avi"`).
    pub format: String,
}

/// Metadata for a video stream.
///
/// Includes dimensions, frame rate, estimated frame count, and codec name.
#[derive(Debug, Clone)]
#[must_use]
pub struct VideoMetadata {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frames per second (may be approximate for variable-frame-rate content).
    pub frames_per_second: f64,
    /// Estimated total number of frames, computed from duration and frame rate.
    pub frame_count: u64,
    /// Codec name (e.g. `"h264"`, `"vp9"`, `"av1"`).
    pub codec: String,
}

/// Metadata for an audio stream.
///
/// Includes sample rate, channel count, codec name, and bit rate.
#[derive(Debug, Clone)]
#[must_use]
pub struct AudioMetadata {
    /// Sample rate in hertz (e.g. `44100`, `48000`).
    pub sample_rate: u32,
    /// Number of audio channels (e.g. `2` for stereo).
    pub channels: u16,
    /// Codec name (e.g. `"aac"`, `"mp3"`, `"flac"`).
    pub codec: String,
    /// Bit rate in bits per second.
    pub bit_rate: u64,
}
