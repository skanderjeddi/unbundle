//! Progress reporting and cancellation support.
//!
//! This module provides [`ProgressCallback`] for monitoring extraction progress,
//! [`CancellationToken`] for cooperative cancellation, and [`ProgressInfo`] for
//! detailed progress snapshots.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use unbundle::{
//!     CancellationToken, ExtractOptions, FrameRange, MediaFile,
//!     OperationType, ProgressCallback, ProgressInfo, UnbundleError,
//! };
//!
//! struct PrintProgress;
//!
//! impl ProgressCallback for PrintProgress {
//!     fn on_progress(&self, info: &ProgressInfo) {
//!         if let Some(pct) = info.percentage {
//!             println!("[{:?}] {pct:.1}% complete", info.operation);
//!         }
//!     }
//! }
//!
//! let mut unbundler = MediaFile::open("input.mp4")?;
//! let config = ExtractOptions::new()
//!     .with_progress(Arc::new(PrintProgress));
//!
//! let frames = unbundler.video().frames_with_options(
//!     FrameRange::Range(0, 99),
//!     &config,
//! )?;
//! # Ok::<(), UnbundleError>(())
//! ```

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

/// The kind of operation currently in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OperationType {
    /// Extracting video frames.
    FrameExtraction,
    /// Extracting / transcoding audio.
    AudioExtraction,
    /// Running scene-change detection.
    SceneDetection,
    /// Validating / analysing a media file.
    Validation,
    /// Remuxing (lossless container format conversion).
    Remuxing,
    /// Extracting subtitle entries.
    SubtitleExtraction,
    /// Transcoding (re-encoding) media.
    Transcoding,
    /// Generating thumbnails.
    ThumbnailGeneration,
    /// Exporting animated GIF.
    GifExport,
    /// Generating audio waveform data.
    WaveformGeneration,
    /// Analysing audio loudness levels.
    LoudnessAnalysis,
    /// Copying stream packets without re-encoding.
    StreamCopy,
}

/// A snapshot of extraction progress.
///
/// Delivered to [`ProgressCallback::on_progress`] at a cadence controlled
/// by [`ExtractOptions::batch_size`](crate::ExtractOptions).
#[derive(Debug, Clone)]
pub struct ProgressInfo {
    /// What kind of work is being performed.
    pub operation: OperationType,
    /// How many items (frames / packets) have been processed so far.
    pub current: u64,
    /// Total items expected, if known ahead of time.
    pub total: Option<u64>,
    /// Completion percentage (0.0 – 100.0), if `total` is known.
    pub percentage: Option<f32>,
    /// Wall-clock time elapsed since the operation started.
    pub elapsed: Duration,
    /// Estimated time remaining, based on current throughput.
    pub estimated_remaining: Option<Duration>,
    /// The frame number currently being processed (video only).
    pub current_frame: Option<u64>,
    /// The timestamp currently being processed.
    pub current_timestamp: Option<Duration>,
}

/// Trait for receiving progress updates during extraction.
///
/// Implementations must be [`Send`] and [`Sync`] because callbacks may be
/// invoked from worker threads in parallel or async contexts.
///
/// Progress callbacks are **infallible** — they observe but cannot halt
/// the operation. Use [`CancellationToken`] for cooperative cancellation.
pub trait ProgressCallback: Send + Sync {
    /// Called at regular intervals during an extraction operation.
    fn on_progress(&self, info: &ProgressInfo);
}

/// A no-op implementation that discards all progress notifications.
///
/// This is the default when no callback is configured.
pub(crate) struct NoOpProgress;

impl ProgressCallback for NoOpProgress {
    fn on_progress(&self, _info: &ProgressInfo) {}
}

/// Cooperative cancellation token backed by an [`AtomicBool`].
///
/// Clone this token and share it between threads; call [`cancel`](CancellationToken::cancel)
/// from any thread to request cancellation of the associated operation.
/// The extraction loop checks [`is_cancelled`](CancellationToken::is_cancelled)
/// before each unit of work.
///
/// # Example
///
/// ```
/// use unbundle::CancellationToken;
///
/// let token = CancellationToken::new();
/// assert!(!token.is_cancelled());
///
/// // From another thread (or a signal handler, etc.):
/// token.cancel();
/// assert!(token.is_cancelled());
/// ```
#[derive(Debug, Clone)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Create a new, non-cancelled token.
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Request cancellation.
    ///
    /// All clones of this token will observe the cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Check whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal helper that tracks progress timing and emits callbacks.
pub(crate) struct ProgressTracker {
    callback: Arc<dyn ProgressCallback>,
    operation: OperationType,
    total: Option<u64>,
    current: u64,
    batch_size: u64,
    start_time: Instant,
    items_since_last_report: u64,
}

impl ProgressTracker {
    /// Create a new tracker.
    pub(crate) fn new(
        callback: Arc<dyn ProgressCallback>,
        operation: OperationType,
        total: Option<u64>,
        batch_size: u64,
    ) -> Self {
        Self {
            callback,
            operation,
            total,
            current: 0,
            batch_size: batch_size.max(1),
            start_time: Instant::now(),
            items_since_last_report: 0,
        }
    }

    /// Record one completed item and fire the callback if the batch
    /// threshold is reached.
    pub(crate) fn advance(&mut self, frame_number: Option<u64>, timestamp: Option<Duration>) {
        self.current += 1;
        self.items_since_last_report += 1;

        if self.items_since_last_report >= self.batch_size {
            self.report(frame_number, timestamp);
            self.items_since_last_report = 0;
        }
    }

    /// Unconditionally emit a final progress report.
    pub(crate) fn finish(&mut self) {
        self.report(None, None);
    }

    fn report(&self, frame_number: Option<u64>, timestamp: Option<Duration>) {
        let elapsed = self.start_time.elapsed();

        let percentage = self
            .total
            .filter(|&t| t > 0)
            .map(|t| (self.current as f32 / t as f32) * 100.0);

        let estimated_remaining = if self.current > 0 {
            self.total.map(|t| {
                let remaining = t.saturating_sub(self.current);
                let per_item = elapsed / self.current as u32;
                per_item * remaining as u32
            })
        } else {
            None
        };

        let info = ProgressInfo {
            operation: self.operation,
            current: self.current,
            total: self.total,
            percentage,
            elapsed,
            estimated_remaining,
            current_frame: frame_number,
            current_timestamp: timestamp,
        };

        self.callback.on_progress(&info);
    }
}
