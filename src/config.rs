//! Extraction configuration.
//!
//! [`ExtractionConfig`] is a builder that threads progress callbacks,
//! cancellation tokens, and other operational settings through extraction
//! methods without polluting every function signature.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use unbundle::{CancellationToken, ExtractionConfig, ProgressCallback, ProgressInfo};
//!
//! struct LogProgress;
//! impl ProgressCallback for LogProgress {
//!     fn on_progress(&self, info: &ProgressInfo) {
//!         println!("{:?}: {} done", info.operation, info.current);
//!     }
//! }
//!
//! let token = CancellationToken::new();
//! let config = ExtractionConfig::new()
//!     .with_progress(Arc::new(LogProgress))
//!     .with_cancellation(token.clone())
//!     .with_batch_size(10);
//! ```

use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::sync::Arc;

use ffmpeg_next::format::Pixel;

use crate::progress::{CancellationToken, NoOpProgress, ProgressCallback};

#[cfg(feature = "hw-accel")]
use crate::hw_accel::HwAccelMode;

/// Output pixel format for extracted frames.
///
/// Controls the colour model and depth of the [`image::DynamicImage`] values
/// returned by video extraction methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PixelFormat {
    /// 8-bit RGB (24 bpp). This is the default.
    #[default]
    Rgb8,
    /// 8-bit RGBA with alpha pre-set to 255 (32 bpp).
    Rgba8,
    /// 8-bit grayscale (8 bpp).
    Gray8,
}

impl PixelFormat {
    /// Map to the corresponding FFmpeg pixel format constant.
    pub(crate) fn to_ffmpeg_pixel(self) -> Pixel {
        match self {
            PixelFormat::Rgb8 => Pixel::RGB24,
            PixelFormat::Rgba8 => Pixel::RGBA,
            PixelFormat::Gray8 => Pixel::GRAY8,
        }
    }
}

/// Frame output settings for video extraction.
///
/// Controls the pixel format and resolution of decoded frames. When no
/// dimensions are set the source resolution is used. Setting one dimension
/// together with [`maintain_aspect_ratio`](FrameOutputConfig::maintain_aspect_ratio)
/// computes the other dimension automatically.
#[derive(Debug, Clone)]
pub struct FrameOutputConfig {
    /// Output pixel format.
    pub pixel_format: PixelFormat,
    /// Target width. `None` keeps the source width.
    pub width: Option<u32>,
    /// Target height. `None` keeps the source height.
    pub height: Option<u32>,
    /// When `true` and only one dimension is specified, the other is
    /// computed to preserve the source aspect ratio.
    pub maintain_aspect_ratio: bool,
}

impl Default for FrameOutputConfig {
    fn default() -> Self {
        Self {
            pixel_format: PixelFormat::Rgb8,
            width: None,
            height: None,
            maintain_aspect_ratio: true,
        }
    }
}

impl FrameOutputConfig {
    /// Resolve the final output dimensions given the source size.
    ///
    /// Returns `(width, height)`.
    pub(crate) fn resolve_dimensions(&self, source_width: u32, source_height: u32) -> (u32, u32) {
        match (self.width, self.height) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) if self.maintain_aspect_ratio && source_width > 0 => {
                let ratio = w as f64 / source_width as f64;
                let h = (source_height as f64 * ratio).round() as u32;
                (w, h.max(1))
            }
            (Some(w), None) => (w, source_height),
            (None, Some(h)) if self.maintain_aspect_ratio && source_height > 0 => {
                let ratio = h as f64 / source_height as f64;
                let w = (source_width as f64 * ratio).round() as u32;
                (w.max(1), h)
            }
            (None, Some(h)) => (source_width, h),
            (None, None) => (source_width, source_height),
        }
    }
}

/// Configuration for extraction operations.
///
/// Carries optional progress-, cancellation-, and tuning-related settings.
/// Pass a reference to this struct to the `*_with_config` methods on
/// [`VideoExtractor`](crate::VideoExtractor) and
/// [`AudioExtractor`](crate::AudioExtractor).
///
/// All fields have sensible defaults — a default-constructed config behaves
/// identically to the original non-config API.
#[derive(Clone)]
pub struct ExtractionConfig {
    /// Progress callback. Defaults to a no-op.
    pub(crate) progress: Arc<dyn ProgressCallback>,
    /// Cancellation token. `None` means never cancelled.
    pub(crate) cancellation: Option<CancellationToken>,
    /// How often to fire the progress callback (every N items).
    /// Defaults to 1 (every item).
    pub(crate) batch_size: u64,
    /// Frame output settings (pixel format, resolution).
    pub(crate) frame_output: FrameOutputConfig,
    /// Hardware acceleration mode (only used when `hw-accel` feature is enabled).
    #[cfg(feature = "hw-accel")]
    pub(crate) hw_accel: HwAccelMode,
}

impl Debug for ExtractionConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("ExtractionConfig")
            .field("has_progress", &true)
            .field("has_cancellation", &self.cancellation.is_some())
            .field("batch_size", &self.batch_size)
            .finish()
    }
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtractionConfig {
    /// Create a new configuration with default settings.
    ///
    /// Defaults: no progress callback, no cancellation, batch size 1.
    pub fn new() -> Self {
        Self {
            progress: Arc::new(NoOpProgress),
            cancellation: None,
            batch_size: 1,
            frame_output: FrameOutputConfig::default(),
            #[cfg(feature = "hw-accel")]
            hw_accel: HwAccelMode::Auto,
        }
    }

    /// Attach a progress callback.
    ///
    /// The callback is invoked every [`batch_size`](ExtractionConfig::with_batch_size)
    /// items during extraction.
    #[must_use]
    pub fn with_progress(mut self, callback: Arc<dyn ProgressCallback>) -> Self {
        self.progress = callback;
        self
    }

    /// Attach a cancellation token.
    ///
    /// When the token is cancelled, the extraction loop will stop and
    /// return [`UnbundleError::Cancelled`](crate::UnbundleError::Cancelled).
    #[must_use]
    pub fn with_cancellation(mut self, token: CancellationToken) -> Self {
        self.cancellation = Some(token);
        self
    }

    /// Set how often the progress callback fires.
    ///
    /// A value of 1 means every item; 10 means every 10th item.
    /// Clamped to a minimum of 1.
    #[must_use]
    pub fn with_batch_size(mut self, size: u64) -> Self {
        self.batch_size = size.max(1);
        self
    }

    /// Set the output pixel format for extracted frames.
    #[must_use]
    pub fn with_pixel_format(mut self, format: PixelFormat) -> Self {
        self.frame_output.pixel_format = format;
        self
    }

    /// Set a custom output resolution for extracted frames.
    ///
    /// Pass `None` for either dimension to keep the source value. When
    /// `maintain_aspect_ratio` is `true` (the default) and only one
    /// dimension is given, the other is computed automatically.
    #[must_use]
    pub fn with_resolution(mut self, width: Option<u32>, height: Option<u32>) -> Self {
        self.frame_output.width = width;
        self.frame_output.height = height;
        self
    }

    /// Control whether aspect ratio is preserved when only one output
    /// dimension is specified. Defaults to `true`.
    #[must_use]
    pub fn with_maintain_aspect_ratio(mut self, maintain: bool) -> Self {
        self.frame_output.maintain_aspect_ratio = maintain;
        self
    }

    /// Set the complete frame output configuration.
    #[must_use]
    pub fn with_frame_output(mut self, config: FrameOutputConfig) -> Self {
        self.frame_output = config;
        self
    }

    /// Set the hardware acceleration mode.
    ///
    /// Only available when the `hw-accel` feature is enabled.
    /// Defaults to [`HwAccelMode::Auto`].
    #[cfg(feature = "hw-accel")]
    #[must_use]
    pub fn with_hw_accel(mut self, mode: HwAccelMode) -> Self {
        self.hw_accel = mode;
        self
    }

    /// Returns `true` if cancellation has been requested.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    }
}
