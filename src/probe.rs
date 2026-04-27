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

#[cfg(feature = "async")]
use crate::stream::MetadataFuture;

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

    /// Probe a media file asynchronously and return its metadata.
    ///
    /// Like [`probe`](MediaProbe::probe), but runs on a blocking thread pool
    /// so it doesn't block the async runtime. The actual probing work is
    /// identical; only the execution context differs.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`probe`](MediaProbe::probe), wrapped in
    /// the future's result.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaProbe, UnbundleError};
    ///
    /// # async fn example() -> Result<(), UnbundleError> {
    /// let metadata = MediaProbe::probe_async("input.mp4").await?;
    /// println!("Duration: {:?}", metadata.duration);
    /// println!("Format: {}", metadata.format);
    /// if let Some(video) = &metadata.video {
    ///     println!("Video: {}x{} @ {} fps", video.width, video.height,
    ///         video.frames_per_second);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "async")]
    pub fn probe_async<P: AsRef<Path>>(path: P) -> MetadataFuture {
        let source = path.as_ref().to_string_lossy().to_string();
        log::debug!("Async probing media file: {}", path.as_ref().display());
        crate::stream::create_metadata_future(source)
    }
}
