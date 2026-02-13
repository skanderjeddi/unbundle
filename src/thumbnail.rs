//! Thumbnail generation utilities.
//!
//! Provides helpers for extracting scaled thumbnails and compositing them
//! into contact-sheet grids. These promote common patterns from user code
//! into the library API.

use std::time::Duration;

use image::{DynamicImage, GenericImage, imageops::FilterType};

use crate::configuration::ExtractOptions;
use crate::error::UnbundleError;
use crate::unbundle::MediaFile;
use crate::video::FrameRange;

/// Options for thumbnail grid generation.
///
/// Controls grid layout, thumbnail dimensions, and spacing.
///
/// # Example
///
/// ```no_run
/// use unbundle::{MediaFile, ThumbnailHandle, ThumbnailOptions, UnbundleError};
///
/// let mut unbundler = MediaFile::open("input.mp4")?;
/// let config = ThumbnailOptions::new(4, 4).with_thumbnail_width(320);
/// let grid = ThumbnailHandle::grid(&mut unbundler, &config)?;
/// grid.save("contact_sheet.png")?;
/// # Ok::<(), UnbundleError>(())
/// ```
#[derive(Debug, Clone)]
#[must_use]
pub struct ThumbnailOptions {
    /// Number of columns in the grid.
    pub columns: u32,
    /// Number of rows in the grid.
    pub rows: u32,
    /// Target width for each thumbnail in pixels.
    ///
    /// The height is computed automatically to preserve aspect ratio.
    pub thumbnail_width: u32,
}

impl ThumbnailOptions {
    /// Create new thumbnail options.
    ///
    /// `columns` and `rows` define the grid dimensions. Thumbnail width
    /// defaults to 320 pixels.
    pub fn new(columns: u32, rows: u32) -> Self {
        Self {
            columns,
            rows,
            thumbnail_width: 320,
        }
    }

    /// Set the target width for each thumbnail.
    ///
    /// Height is derived automatically from the video's aspect ratio.
    pub fn with_thumbnail_width(mut self, width: u32) -> Self {
        self.thumbnail_width = width;
        self
    }

    /// Set the target width for each thumbnail.
    ///
    /// Alias for [`with_thumbnail_width`](ThumbnailOptions::with_thumbnail_width).
    pub fn thumbnail_width(self, width: u32) -> Self {
        self.with_thumbnail_width(width)
    }
}

/// Thumbnail generation utilities.
///
/// All methods are stateless functions that accept a
/// [`MediaFile`] reference.
///
/// # Example
///
/// ```no_run
/// use std::time::Duration;
///
/// use unbundle::{MediaFile, ThumbnailHandle, ThumbnailOptions, UnbundleError};
///
/// let mut unbundler = MediaFile::open("input.mp4")?;
///
/// // Single thumbnail at 10 seconds, max 640px on longest edge
/// let thumb = ThumbnailHandle::at_timestamp(
///     &mut unbundler,
///     Duration::from_secs(10),
///     640,
/// )?;
/// thumb.save("thumb.jpg")?;
///
/// // Contact-sheet grid
/// let config = ThumbnailOptions::new(4, 4);
/// let grid = ThumbnailHandle::grid(&mut unbundler, &config)?;
/// grid.save("grid.png")?;
/// # Ok::<(), UnbundleError>(())
/// ```
pub struct ThumbnailHandle;

impl ThumbnailHandle {
    /// Extract a single thumbnail at a timestamp, scaled to fit within
    /// `max_dimension` on its longest edge.
    ///
    /// Preserves the video's aspect ratio. For example, a 1920×1080 frame
    /// with `max_dimension = 640` produces a 640×360 thumbnail.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::NoVideoStream`] if the file has no video,
    /// [`UnbundleError::InvalidTimestamp`] if the timestamp exceeds the
    /// duration, or decoding errors.
    pub fn at_timestamp(
        unbundler: &mut MediaFile,
        timestamp: Duration,
        max_dimension: u32,
    ) -> Result<DynamicImage, UnbundleError> {
        log::debug!(
            "Generating thumbnail at {:?} (max_dim={})",
            timestamp,
            max_dimension
        );
        let image = unbundler.video().frame_at(timestamp)?;
        let (width, height) = (image.width(), image.height());
        let (thumb_width, thumb_height) = fit_dimensions(width, height, max_dimension);
        Ok(image.resize_exact(thumb_width, thumb_height, FilterType::Triangle))
    }

    /// Extract a single thumbnail at a frame number, scaled to fit within
    /// `max_dimension` on its longest edge.
    ///
    /// # Errors
    ///
    /// Same as [`at_timestamp`](ThumbnailHandle::at_timestamp).
    pub fn at_frame(
        unbundler: &mut MediaFile,
        frame_number: u64,
        max_dimension: u32,
    ) -> Result<DynamicImage, UnbundleError> {
        let image = unbundler.video().frame(frame_number)?;
        let (width, height) = (image.width(), image.height());
        let (thumb_width, thumb_height) = fit_dimensions(width, height, max_dimension);
        Ok(image.resize_exact(thumb_width, thumb_height, FilterType::Triangle))
    }

    /// Generate a thumbnail contact-sheet grid.
    ///
    /// Extracts `columns × rows` frames at evenly-spaced intervals across
    /// the video, scales them to the configured thumbnail width (preserving
    /// aspect ratio), and composites them into a single image.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::NoVideoStream`] if the file has no video, or
    /// decoding / image errors.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, ThumbnailHandle, ThumbnailOptions, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let config = ThumbnailOptions::new(4, 4).with_thumbnail_width(240);
    /// let grid = ThumbnailHandle::grid(&mut unbundler, &config)?;
    /// grid.save("contact_sheet.png")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn grid(
        unbundler: &mut MediaFile,
        config: &ThumbnailOptions,
    ) -> Result<DynamicImage, UnbundleError> {
        Self::grid_with_options(unbundler, config, &ExtractOptions::default())
    }

    /// Generate a thumbnail grid with progress/cancellation support.
    ///
    /// Like [`grid`](ThumbnailHandle::grid) but accepts an
    /// [`ExtractOptions`] for progress callbacks and cancellation.
    pub fn grid_with_options(
        unbundler: &mut MediaFile,
        config: &ThumbnailOptions,
        extraction_config: &ExtractOptions,
    ) -> Result<DynamicImage, UnbundleError> {
        log::debug!(
            "Generating {}x{} thumbnail grid (thumb_width={})",
            config.columns,
            config.rows,
            config.thumbnail_width
        );
        let video_metadata = unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let total_thumbnails = config.columns * config.rows;
        let frame_count = video_metadata.frame_count;

        // Compute evenly-spaced frame numbers.
        let step = if frame_count > total_thumbnails as u64 {
            frame_count / total_thumbnails as u64
        } else {
            1
        };
        let frame_numbers: Vec<u64> = (0..total_thumbnails as u64)
            .map(|index| index * step)
            .filter(|number| *number < frame_count)
            .collect();

        let frames = unbundler
            .video()
            .frames_with_options(FrameRange::Specific(frame_numbers), extraction_config)?;

        // Compute thumbnail dimensions preserving aspect ratio.
        let scale_factor = config.thumbnail_width as f64 / video_metadata.width as f64;
        let scaled_width = config.thumbnail_width;
        let scaled_height = (video_metadata.height as f64 * scale_factor).round() as u32;

        // Composite the grid.
        let grid_width = scaled_width * config.columns;
        let grid_height = scaled_height * config.rows;
        let mut grid = DynamicImage::new_rgb8(grid_width, grid_height);

        for (index, frame) in frames.iter().enumerate() {
            let column = (index as u32) % config.columns;
            let row = (index as u32) / config.columns;
            if row >= config.rows {
                break;
            }

            let thumbnail = frame.resize_exact(scaled_width, scaled_height, FilterType::Triangle);

            let x = column * scaled_width;
            let y = row * scaled_height;
            // copy_from can fail if dimensions mismatch — should not happen here.
            let _ = grid.copy_from(&thumbnail, x, y);
        }

        Ok(grid)
    }

    /// Extract a "smart" thumbnail that avoids black or near-uniform frames.
    ///
    /// Samples `sample_count` frames evenly across the video and picks the
    /// one with the highest pixel variance (most visual detail). The chosen
    /// frame is then scaled to fit within `max_dimension`.
    ///
    /// This is useful for generating representative thumbnails without
    /// relying on a fixed timestamp that might land on a fade-to-black or
    /// title card.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::NoVideoStream`] if the file has no video, or
    /// decoding errors.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, ThumbnailHandle, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let thumb = ThumbnailHandle::smart(&mut unbundler, 20, 640)?;
    /// thumb.save("smart_thumb.jpg")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn smart(
        unbundler: &mut MediaFile,
        sample_count: u32,
        max_dimension: u32,
    ) -> Result<DynamicImage, UnbundleError> {
        Self::smart_with_options(
            unbundler,
            sample_count,
            max_dimension,
            &ExtractOptions::default(),
        )
    }

    /// Extract a smart thumbnail with progress/cancellation support.
    ///
    /// Like [`smart`](ThumbnailHandle::smart) but accepts an
    /// [`ExtractOptions`] for progress callbacks and cancellation.
    pub fn smart_with_options(
        unbundler: &mut MediaFile,
        sample_count: u32,
        max_dimension: u32,
        extraction_config: &ExtractOptions,
    ) -> Result<DynamicImage, UnbundleError> {
        log::debug!(
            "Generating smart thumbnail (samples={}, max_dim={})",
            sample_count,
            max_dimension
        );
        let video_metadata = unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let frame_count = video_metadata.frame_count;
        let count = (sample_count as u64).min(frame_count).max(1);

        let step = if frame_count > count {
            frame_count / count
        } else {
            1
        };
        let frame_numbers: Vec<u64> = (0..count)
            .map(|i| i * step)
            .filter(|n| *n < frame_count)
            .collect();

        // Extract with a small resolution for fast variance computation.
        // We use the caller's config for cancellation/progress support.
        let frames = unbundler.video().frames_with_options(
            FrameRange::Specific(frame_numbers.clone()),
            extraction_config,
        )?;

        // Find the frame with highest pixel variance.
        let mut best_index = 0;
        let mut best_variance: f64 = -1.0;

        for (index, frame) in frames.iter().enumerate() {
            let variance = pixel_variance(frame);
            if variance > best_variance {
                best_variance = variance;
                best_index = index;
            }
        }

        // Re-extract the winning frame at full resolution.
        let best_frame_number = frame_numbers.get(best_index).copied().unwrap_or(0);
        let full_image = unbundler.video().frame(best_frame_number)?;
        let (width, height) = (full_image.width(), full_image.height());
        let (thumb_width, thumb_height) = fit_dimensions(width, height, max_dimension);

        Ok(full_image.resize_exact(thumb_width, thumb_height, FilterType::Triangle))
    }
}

/// Compute dimensions that fit within `max_dimension` preserving aspect ratio.
fn fit_dimensions(width: u32, height: u32, max_dimension: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (max_dimension, max_dimension);
    }
    let scale = max_dimension as f64 / width.max(height) as f64;
    let new_width = ((width as f64) * scale).round() as u32;
    let new_height = ((height as f64) * scale).round() as u32;
    (new_width.max(1), new_height.max(1))
}

/// Compute the pixel variance of an image (higher = more visual detail).
///
/// Uses the grayscale luminance for speed. Returns the variance of pixel
/// values across the entire image.
fn pixel_variance(image: &DynamicImage) -> f64 {
    let gray = image.to_luma8();
    let pixels = gray.as_raw();
    if pixels.is_empty() {
        return 0.0;
    }
    let count = pixels.len() as f64;
    let mean: f64 = pixels.iter().map(|&p| p as f64).sum::<f64>() / count;
    let variance: f64 = pixels
        .iter()
        .map(|&p| {
            let diff = p as f64 - mean;
            diff * diff
        })
        .sum::<f64>()
        / count;
    variance
}
