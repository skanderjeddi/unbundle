//! Audio transcoding (re-encoding between formats).
//!
//! This module provides [`Transcoder`] for re-encoding audio from one
//! codec/format to another.  Unlike [`Remuxer`](crate::Remuxer) which
//! copies packets verbatim, the transcoder decodes and re-encodes the
//! audio stream, allowing codec changes (e.g. AAC → MP3).
//!
//! Video and subtitle streams are **not** included in the output.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{AudioFormat, MediaFile, Transcoder, UnbundleError};
//!
//! let mut unbundler = MediaFile::open("input.mp4")?;
//! Transcoder::new(&mut unbundler)
//!     .format(AudioFormat::Mp3)
//!     .run("output.mp3")?;
//! # Ok::<(), UnbundleError>(())
//! ```

use std::path::Path;
use std::time::Duration;

use crate::audio::AudioFormat;
use crate::error::UnbundleError;
use crate::unbundle::MediaFile;

/// Builder for audio transcoding operations.
///
/// Obtained via [`Transcoder::new`].  Configure the target format,
/// bitrate, and optional time range, then call [`run`](Transcoder::run)
/// to produce the output file.
pub struct Transcoder<'a> {
    unbundler: &'a mut MediaFile,
    format: AudioFormat,
    start: Option<Duration>,
    end: Option<Duration>,
    bitrate: Option<usize>,
}

impl<'a> Transcoder<'a> {
    /// Create a new transcoder for the given unbundler.
    ///
    /// The default output format is WAV.
    pub fn new(unbundler: &'a mut MediaFile) -> Self {
        Self {
            unbundler,
            format: AudioFormat::Wav,
            start: None,
            end: None,
            bitrate: None,
        }
    }

    /// Set the target audio format.
    pub fn format(mut self, format: AudioFormat) -> Self {
        self.format = format;
        self
    }

    /// Set an optional start time for the transcoded range.
    pub fn start(mut self, start: Duration) -> Self {
        self.start = Some(start);
        self
    }

    /// Set an optional end time for the transcoded range.
    pub fn end(mut self, end: Duration) -> Self {
        self.end = Some(end);
        self
    }

    /// Set the target bitrate in bits per second. If not set, the encoder
    /// default is used.
    pub fn bitrate(mut self, bitrate: usize) -> Self {
        self.bitrate = Some(bitrate);
        self
    }

    /// Run the transcode and write the output to `path`.
    ///
    /// This delegates to `AudioHandle::save_range` (or `save`) under
    /// the hood: the audio is decoded and re-encoded to the target format.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoAudioStream`] if no audio stream exists.
    /// - [`UnbundleError::TranscodeError`] if encoding fails.
    pub fn run<P: AsRef<Path>>(self, path: P) -> Result<(), UnbundleError> {
        log::info!(
            "Transcoding audio to {:?} (format={:?})",
            path.as_ref(),
            self.format
        );
        match (self.start, self.end) {
            (Some(start), Some(end)) => self
                .unbundler
                .audio()
                .save_range(path, start, end, self.format)
                .map_err(|e| UnbundleError::TranscodeError(e.to_string())),
            _ => self
                .unbundler
                .audio()
                .save(path, self.format)
                .map_err(|e| UnbundleError::TranscodeError(e.to_string())),
        }
    }

    /// Run the transcode and return the encoded bytes in memory.
    ///
    /// # Errors
    ///
    /// Same as [`run`](Transcoder::run).
    pub fn run_to_memory(self) -> Result<Vec<u8>, UnbundleError> {
        log::debug!("Transcoding audio to memory (format={:?})", self.format);
        match (self.start, self.end) {
            (Some(start), Some(end)) => self
                .unbundler
                .audio()
                .extract_range(start, end, self.format)
                .map_err(|e| UnbundleError::TranscodeError(e.to_string())),
            _ => self
                .unbundler
                .audio()
                .extract(self.format)
                .map_err(|e| UnbundleError::TranscodeError(e.to_string())),
        }
    }
}
