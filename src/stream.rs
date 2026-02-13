//! Async streaming for video and audio extraction.
//!
//! This module provides [`FrameStream`] for asynchronously iterating over
//! decoded video frames, and [`AudioFuture`] for extracting audio data
//! in the background without blocking the async runtime.
//!
//! Both types use `tokio::task::spawn_blocking` internally — decoding happens
//! on a dedicated blocking thread while results are streamed back through a
//! bounded channel. This avoids tying up the Tokio runtime's cooperative
//! task budget with CPU-heavy FFmpeg work.
//!
//! # Example
//!
//! ```no_run
//! use tokio_stream::StreamExt;
//!
//! use unbundle::{ExtractOptions, FrameRange, MediaFile, UnbundleError};
//!
//! # async fn example() -> Result<(), UnbundleError> {
//! let mut unbundler = MediaFile::open("input.mp4")?;
//! let config = ExtractOptions::new();
//! let mut stream = unbundler
//!     .video()
//!     .frame_stream(FrameRange::Range(0, 9), config)?;
//!
//! while let Some(result) = stream.next().await {
//!     let (frame_number, image) = result?;
//!     image.save(format!("frame_{frame_number}.png"))?;
//! }
//! # Ok(())
//! # }
//! ```

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use image::DynamicImage;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use tokio_stream::Stream;

use crate::audio::AudioFormat;
use crate::configuration::ExtractOptions;
use crate::error::UnbundleError;
use crate::unbundle::MediaFile;
use crate::video::FrameRange;

/// Default bounded-channel capacity for [`FrameStream`].
///
/// Kept small to avoid buffering too many large decoded frames in memory.
const DEFAULT_CHANNEL_CAPACITY: usize = 8;

/// A stream of decoded video frames produced by a background decode thread.
///
/// Implements [`tokio_stream::Stream`] so it can be used with
/// [`StreamExt`](tokio_stream::StreamExt) combinators such as `next()`,
/// `map()`, `filter()`, and `take()`.
///
/// The background decoder is spawned via `tokio::task::spawn_blocking` and
/// communicates through a bounded `mpsc` channel. Dropping the stream
/// closes the channel, which causes the background thread to stop at the
/// next frame boundary.
///
/// # Example
///
/// ```no_run
/// use tokio_stream::StreamExt;
///
/// use unbundle::{ExtractOptions, FrameRange, MediaFile, UnbundleError};
///
/// # async fn example() -> Result<(), UnbundleError> {
/// let mut unbundler = MediaFile::open("input.mp4")?;
/// let mut stream = unbundler
///     .video()
///     .frame_stream(FrameRange::Interval(30), ExtractOptions::new())?;
///
/// while let Some(result) = stream.next().await {
///     let (frame_number, image) = result?;
///     println!("Got frame {frame_number}");
/// }
/// # Ok(())
/// # }
/// ```
pub struct FrameStream {
    receiver: Receiver<Result<(u64, DynamicImage), UnbundleError>>,
    #[allow(dead_code)]
    handle: JoinHandle<()>,
}

impl Stream for FrameStream {
    type Item = Result<(u64, DynamicImage), UnbundleError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}

/// Create a [`FrameStream`] that decodes video frames on a blocking thread.
///
/// Opens a fresh demuxer for `source`, decodes frames matching `range`,
/// and sends each `(frame_number, DynamicImage)` through a bounded channel.
///
/// # Arguments
///
/// * `source` — Input source string (path/URL) cloned from the original unbundler.
/// * `range` — Which frames to decode.
/// * `config` — Extraction settings (progress, cancellation, output format).
/// * `channel_capacity` — Bounded channel size. `None` uses the default (8).
pub(crate) fn create_frame_stream(
    source: String,
    range: FrameRange,
    config: ExtractOptions,
    channel_capacity: Option<usize>,
) -> FrameStream {
    let capacity = channel_capacity.unwrap_or(DEFAULT_CHANNEL_CAPACITY).max(1);
    let (sender, receiver) = tokio::sync::mpsc::channel(capacity);

    let handle = tokio::task::spawn_blocking(move || {
        let result = decode_frames_blocking(&source, range, &config, &sender);
        if let Err(e) = result {
            // Try to send the error; the receiver may have been dropped.
            let _ = sender.blocking_send(Err(e));
        }
    });

    FrameStream { receiver, handle }
}

/// Background decode loop — runs on a blocking thread.
fn decode_frames_blocking(
    source: &str,
    range: FrameRange,
    config: &ExtractOptions,
    sender: &Sender<Result<(u64, DynamicImage), UnbundleError>>,
) -> Result<(), UnbundleError> {
    let mut unbundler = MediaFile::open_source(source)?;

    unbundler
        .video()
        .for_each_frame_with_options(range, config, |frame_number, image| {
            sender
                .blocking_send(Ok((frame_number, image)))
                .map_err(|_| UnbundleError::Cancelled)
        })
}

/// A future that resolves to extracted audio data.
///
/// Created via [`AudioHandle::extract_async`](crate::AudioHandle) or
/// similar async audio methods. The actual transcoding runs on a blocking
/// thread; polling this future drives it to completion.
///
/// # Example
///
/// ```no_run
/// use unbundle::{AudioFormat, ExtractOptions, MediaFile, UnbundleError};
///
/// # async fn example() -> Result<(), UnbundleError> {
/// let mut unbundler = MediaFile::open("input.mp4")?;
/// let config = ExtractOptions::new();
/// let audio_bytes = unbundler
///     .audio()
///     .extract_async(AudioFormat::Wav, config)?
///     .await?;
/// println!("Got {} bytes of audio", audio_bytes.len());
/// # Ok(())
/// # }
/// ```
pub struct AudioFuture {
    handle: JoinHandle<Result<Vec<u8>, UnbundleError>>,
}

impl Future for AudioFuture {
    type Output = Result<Vec<u8>, UnbundleError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.handle)
            .poll(cx)
            .map(|result| result.unwrap_or_else(|_| Err(UnbundleError::Cancelled)))
    }
}

/// Create an [`AudioFuture`] that transcodes audio on a blocking thread.
///
/// Opens a fresh demuxer for `source` and extracts the specified audio
/// track in the given format.
pub(crate) fn create_audio_future(
    source: String,
    format: AudioFormat,
    track_index: Option<usize>,
    range: Option<(Duration, Duration)>,
    config: ExtractOptions,
) -> AudioFuture {
    let handle = tokio::task::spawn_blocking(move || {
        let mut unbundler = MediaFile::open_source(&source)?;

        let mut extractor = if let Some(index) = track_index {
            unbundler.audio_track(index)?
        } else {
            unbundler.audio()
        };

        match range {
            Some((start, end)) => extractor.extract_range_with_options(start, end, format, &config),
            None => extractor.extract_with_options(format, &config),
        }
    });

    AudioFuture { handle }
}
