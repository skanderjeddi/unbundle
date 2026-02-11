//! Error types for the `unbundle` crate.
//!
//! This module defines [`UnbundleError`], the unified error type returned by all
//! fallible operations in the crate. Errors carry rich context to aid debugging,
//! including file paths, frame numbers, and upstream error messages.

use std::{io::Error as IoError, path::PathBuf, time::Duration};

use ffmpeg_next::Error as FfmpegError;
use image::ImageError;
use thiserror::Error;

use crate::audio::AudioFormat;

/// The unified error type for all `unbundle` operations.
///
/// Every public method that can fail returns `Result<T, UnbundleError>`.
/// Variants carry enough context to diagnose the problem without needing
/// additional logging at the call site.
#[derive(Debug, Error)]
pub enum UnbundleError {
    /// The media file could not be opened.
    #[error("Failed to open media file at {path}: {reason}")]
    FileOpen {
        /// Path that was passed to [`crate::MediaUnbundler::open`].
        path: PathBuf,
        /// Underlying reason the open failed.
        reason: String,
    },

    /// The file does not contain a video stream.
    #[error("No video stream found in file")]
    NoVideoStream,

    /// The file does not contain an audio stream.
    #[error("No audio stream found in file")]
    NoAudioStream,

    /// A video frame could not be decoded.
    #[error("Failed to decode video frame: {0}")]
    VideoDecodeError(String),

    /// Audio data could not be decoded.
    #[error("Failed to decode audio: {0}")]
    AudioDecodeError(String),

    /// Audio data could not be encoded to the target format.
    #[error("Failed to encode audio: {0}")]
    AudioEncodeError(String),

    /// The requested frame number exceeds the total frame count.
    #[error("Frame {frame_number} is out of range (video has {total_frames} frames)")]
    FrameOutOfRange {
        /// The frame number that was requested.
        frame_number: u64,
        /// The total number of frames in the video.
        total_frames: u64,
    },

    /// The requested timestamp exceeds the media duration.
    #[error("Invalid timestamp: {0:?}")]
    InvalidTimestamp(Duration),

    /// The requested audio output format is not supported.
    #[error("Unsupported audio format: {0:?}")]
    UnsupportedAudioFormat(AudioFormat),

    /// An error originating from the FFmpeg libraries.
    #[error("FFmpeg error: {0}")]
    FfmpegError(String),

    /// An I/O error occurred while reading or writing files.
    #[error("I/O error: {0}")]
    IoError(#[from] IoError),

    /// An error from the `image` crate during frame conversion.
    #[error("Image processing error: {0}")]
    ImageError(#[from] ImageError),
}

impl From<FfmpegError> for UnbundleError {
    fn from(error: FfmpegError) -> Self {
        UnbundleError::FfmpegError(error.to_string())
    }
}
