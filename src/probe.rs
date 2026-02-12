//! Lightweight media file probing.
//!
//! [`MediaProbe`] extracts metadata from a media file without keeping the
//! demuxer open. This is useful for quickly inspecting many files (e.g. in a
//! directory listing) without the cost of retaining an FFmpeg input context
//! per file.
//!
//! For full extraction capabilities, use
//! [`MediaFile::open`](crate::MediaFile::open) instead.

use std::path::Path;

use crate::error::UnbundleError;
use crate::metadata::MediaMetadata;
use crate::unbundle::MediaFile;

/// Lightweight media file probe.
///
/// Opens the file, extracts metadata, and immediately closes the demuxer.
/// The resulting [`MediaMetadata`] is identical to what
/// [`MediaFile::metadata`](crate::MediaFile::metadata) returns, but
/// without keeping the file open for extraction.
///
/// # Example
///
/// ```no_run
/// use unbundle::{MediaProbe, UnbundleError};
///
/// let metadata = MediaProbe::probe("input.mp4")?;
/// println!("Duration: {:?}, format: {}", metadata.duration, metadata.format);
/// if let Some(video) = &metadata.video {
///     println!("Video: {}x{} @ {} fps", video.width, video.height, video.frames_per_second);
/// }
/// # Ok::<(), UnbundleError>(())
/// ```
pub struct MediaProbe;

impl MediaProbe {
    /// Probe a media file and return its metadata.
    ///
    /// Opens the file, extracts all available metadata (video, audio,
    /// subtitle streams, chapters), and closes the demuxer. The returned
    /// [`MediaMetadata`] is owned and fully independent of any file handle.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::FileOpen`] if the file cannot be opened or
    /// recognised as a media file.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaProbe, UnbundleError};
    ///
    /// let metadata = MediaProbe::probe("video.mkv")?;
    /// println!("{:?}", metadata);
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn probe<P: AsRef<Path>>(path: P) -> Result<MediaMetadata, UnbundleError> {
        log::debug!("Probing media file: {}", path.as_ref().display());
        let unbundler = MediaFile::open(path)?;
        Ok(unbundler.metadata.clone())
    }

    /// Probe multiple media files and return their metadata.
    ///
    /// Files that cannot be probed produce an `Err` entry in the result
    /// vector rather than aborting the entire batch.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::MediaProbe;
    ///
    /// let results = MediaProbe::probe_many(&["a.mp4", "b.mkv", "c.avi"]);
    /// for result in &results {
    ///     match result {
    ///         Ok(meta) => println!("{}: {:?}", meta.format, meta.duration),
    ///         Err(err) => eprintln!("Error: {err}"),
    ///     }
    /// }
    /// ```
    pub fn probe_many<P: AsRef<Path>>(paths: &[P]) -> Vec<Result<MediaMetadata, UnbundleError>> {
        paths.iter().map(|path| Self::probe(path)).collect()
    }
}
