//! GIF export from video frames.
//!
//! This module provides [`GifOptions`] for configuring animated GIF output
//! and the internal encoding logic used by
//! [`VideoHandle::export_gif`](crate::VideoHandle).
//!
//! # Example
//!
//! ```no_run
//! use std::time::Duration;
//! use unbundle::{FrameRange, GifOptions, MediaFile, UnbundleError};
//!
//! let mut unbundler = MediaFile::open("input.mp4")?;
//! let config = GifOptions::new()
//!     .width(320)
//!     .frame_delay(100);
//!
//! unbundler.video().export_gif(
//!     "output.gif",
//!     FrameRange::TimeRange(Duration::from_secs(0), Duration::from_secs(5)),
//!     &config,
//! )?;
//! # Ok::<(), UnbundleError>(())
//! ```

use std::fs::File;
use std::path::Path;

use gif::{Encoder, Frame, Repeat};
use image::DynamicImage;

use crate::configuration::{FrameOutputOptions, PixelFormat};
use crate::error::UnbundleError;

/// Configuration for animated GIF export.
///
/// Controls output dimensions, frame delay, repeat behaviour, and quality.
#[derive(Debug, Clone)]
pub struct GifOptions {
    /// Target width in pixels. Height is computed to preserve aspect ratio.
    /// `None` means use source resolution.
    pub width: Option<u32>,
    /// Delay between frames in hundredths of a second (default: 10 = 100 ms).
    pub frame_delay: u16,
    /// How many times the GIF should repeat. `None` means loop forever.
    pub repeat: Option<u16>,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            width: None,
            frame_delay: 10,
            repeat: None,
        }
    }
}

impl GifOptions {
    /// Create a new [`GifOptions`] with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the target width (height is auto-scaled to preserve aspect ratio).
    pub fn width(mut self, width: u32) -> Self {
        self.width = Some(width);
        self
    }

    /// Set the target width (height is auto-scaled to preserve aspect ratio).
    ///
    /// Alias for [`width`](GifOptions::width).
    pub fn with_width(self, width: u32) -> Self {
        self.width(width)
    }

    /// Set the delay between frames in hundredths of a second.
    ///
    /// For example, `10` = 100 ms between frames ≈ 10 fps.
    pub fn frame_delay(mut self, delay: u16) -> Self {
        self.frame_delay = delay;
        self
    }

    /// Set the delay between frames in hundredths of a second.
    ///
    /// Alias for [`frame_delay`](GifOptions::frame_delay).
    pub fn with_frame_delay(self, delay: u16) -> Self {
        self.frame_delay(delay)
    }

    /// Set the repeat count. `None` means loop forever.
    pub fn repeat(mut self, repeat: Option<u16>) -> Self {
        self.repeat = repeat;
        self
    }

    /// Set the repeat count. `None` means loop forever.
    ///
    /// Alias for [`repeat`](GifOptions::repeat).
    pub fn with_repeat(self, repeat: Option<u16>) -> Self {
        self.repeat(repeat)
    }

    /// Build a [`FrameOutputOptions`] matching this GIF configuration.
    pub(crate) fn to_frame_output_config(
        &self,
        _source_width: u32,
        _source_height: u32,
    ) -> FrameOutputOptions {
        let mut frame_output = FrameOutputOptions::default();
        // GIF is always RGBA8 for transparency / palette handling.
        frame_output.pixel_format = PixelFormat::Rgba8;
        if let Some(width) = self.width {
            frame_output.width = Some(width);
        }
        frame_output
    }
}

/// Encode a sequence of frames as an animated GIF to the given path.
///
/// Each frame is quantized to a 256-colour palette using the `gif` crate's
/// built-in quantiser.
pub(crate) fn encode_gif<P: AsRef<Path>>(
    path: P,
    frames: &[DynamicImage],
    config: &GifOptions,
) -> Result<(), UnbundleError> {
    log::debug!(
        "Encoding {} frames to GIF file {:?} (width={:?}, delay={})",
        frames.len(),
        path.as_ref(),
        config.width,
        config.frame_delay,
    );
    if frames.is_empty() {
        return Ok(());
    }

    let first = &frames[0];
    let width = first.width() as u16;
    let height = first.height() as u16;

    let file = File::create(path.as_ref())
        .map_err(|e| UnbundleError::GifEncodeError(format!("Failed to create GIF file: {e}")))?;

    let mut encoder = Encoder::new(file, width, height, &[])
        .map_err(|e| UnbundleError::GifEncodeError(format!("Failed to create GIF encoder: {e}")))?;

    let repeat = match config.repeat {
        None => Repeat::Infinite,
        Some(n) => Repeat::Finite(n),
    };
    encoder
        .set_repeat(repeat)
        .map_err(|e| UnbundleError::GifEncodeError(format!("Failed to set GIF repeat: {e}")))?;

    for image in frames {
        let rgba = image.to_rgba8();
        let mut pixels = rgba.into_raw();

        let mut gif_frame = Frame::from_rgba_speed(width, height, &mut pixels, 10);
        gif_frame.delay = config.frame_delay;

        encoder.write_frame(&gif_frame).map_err(|e| {
            UnbundleError::GifEncodeError(format!("Failed to write GIF frame: {e}"))
        })?;
    }

    Ok(())
}

/// Encode a sequence of frames as an animated GIF into memory.
///
/// Returns the raw GIF bytes.
pub(crate) fn encode_gif_to_memory(
    frames: &[DynamicImage],
    config: &GifOptions,
) -> Result<Vec<u8>, UnbundleError> {
    log::debug!(
        "Encoding {} frames to GIF in memory (width={:?}, delay={})",
        frames.len(),
        config.width,
        config.frame_delay,
    );
    if frames.is_empty() {
        return Ok(Vec::new());
    }

    let first = &frames[0];
    let width = first.width() as u16;
    let height = first.height() as u16;

    let mut buffer = Vec::new();

    {
        let mut encoder = Encoder::new(&mut buffer, width, height, &[]).map_err(|e| {
            UnbundleError::GifEncodeError(format!("Failed to create GIF encoder: {e}"))
        })?;

        let repeat = match config.repeat {
            None => Repeat::Infinite,
            Some(n) => Repeat::Finite(n),
        };
        encoder
            .set_repeat(repeat)
            .map_err(|e| UnbundleError::GifEncodeError(format!("Failed to set GIF repeat: {e}")))?;

        for image in frames {
            let rgba = image.to_rgba8();
            let mut pixels = rgba.into_raw();

            let mut gif_frame = Frame::from_rgba_speed(width, height, &mut pixels, 10);
            gif_frame.delay = config.frame_delay;

            encoder.write_frame(&gif_frame).map_err(|e| {
                UnbundleError::GifEncodeError(format!("Failed to write GIF frame: {e}"))
            })?;
        }
    }

    Ok(buffer)
}
