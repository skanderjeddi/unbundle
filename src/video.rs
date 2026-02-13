//! Video frame extraction.
//!
//! This module provides [`VideoHandle`] for extracting still frames from
//! video files, and [`FrameRange`] for specifying which frames to extract.
//! Extracted frames are returned as [`image::DynamicImage`] values that can be
//! saved, manipulated, or converted to other formats.

use std::path::Path;
use std::time::Duration;

use ffmpeg_next::{
    Rational,
    codec::context::Context as CodecContext,
    decoder::Video as VideoDecoder,
    format::Pixel,
    frame::Video as VideoFrame,
    software::scaling::{Context as ScalingContext, Flags as ScalingFlags},
    util::picture::Type as PictureType,
};
use image::{DynamicImage, GrayImage, RgbImage, RgbaImage};

#[cfg(feature = "gif")]
use crate::gif::GifOptions;
use crate::keyframe::{GroupOfPicturesInfo, KeyFrameMetadata};
#[cfg(feature = "scene")]
use crate::scene::{SceneChange, SceneDetectionOptions};
#[cfg(feature = "async")]
use crate::stream::FrameStream;
use crate::variable_framerate::VariableFrameRateAnalysis;
use crate::{
    configuration::{ExtractOptions, FrameOutputOptions, PixelFormat},
    error::UnbundleError,
    metadata::VideoMetadata,
    progress::{OperationType, ProgressTracker},
    unbundle::MediaFile,
    video_iterator::FrameIterator,
};

/// The type of a decoded video frame (I, P, B, etc.).
///
/// Maps to FFmpeg's `AVPictureType`. Most applications only care about
/// [`FrameType::I`] (keyframes), [`FrameType::P`] (predicted), and
/// [`FrameType::B`] (bi-predicted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    /// Intra-coded frame (keyframe).
    I,
    /// Predicted frame.
    P,
    /// Bi-directionally predicted frame.
    B,
    /// S(GMC)-VOP MPEG-4 frame.
    S,
    /// Switching intra frame.
    SI,
    /// Switching predicted frame.
    SP,
    /// BI frame.
    BI,
    /// Unknown or unset picture type.
    Unknown,
}

/// Metadata about a single decoded video frame.
///
/// Returned alongside the decoded [`DynamicImage`] by methods such as
/// [`VideoHandle::frame_and_metadata`] and
/// [`VideoHandle::frames_and_metadata`].
///
/// # Example
///
/// ```no_run
/// use unbundle::{MediaFile, UnbundleError};
///
/// let mut unbundler = MediaFile::open("input.mp4")?;
/// let (image, info) = unbundler.video().frame_and_metadata(0)?;
/// println!("Frame {} at {:?}, keyframe={}", info.frame_number,
///     info.timestamp, info.is_keyframe);
/// # Ok::<(), UnbundleError>(())
/// ```
#[derive(Debug, Clone)]
pub struct FrameMetadata {
    /// The zero-indexed frame number within the video.
    pub frame_number: u64,
    /// Presentation timestamp as a [`Duration`] from the start of the video.
    pub timestamp: Duration,
    /// Raw PTS value in stream time-base units, if available.
    pub pts: Option<i64>,
    /// Whether this frame is a keyframe (I-frame).
    pub is_keyframe: bool,
    /// The picture type (I, P, B, etc.) of the decoded frame.
    pub frame_type: FrameType,
}

/// Specifies which frames to extract from a video.
///
/// Used with [`VideoHandle::frames`] to extract multiple frames in a single
/// call.
///
/// # Example
///
/// ```no_run
/// use std::time::Duration;
///
/// use unbundle::{FrameRange, MediaFile};
///
/// let mut unbundler = MediaFile::open("input.mp4").unwrap();
///
/// // Extract every 30th frame
/// let frames = unbundler.video().frames(FrameRange::Interval(30)).unwrap();
/// ```
#[derive(Debug, Clone)]
#[must_use]
pub enum FrameRange {
    /// Extract frames from start to end (inclusive, 0-indexed).
    Range(u64, u64),
    /// Extract every Nth frame from the entire video.
    Interval(u64),
    /// Extract all frames between two timestamps.
    TimeRange(Duration, Duration),
    /// Extract frames at regular time intervals (e.g. every 2 seconds).
    TimeInterval(Duration),
    /// Extract frames at specific frame numbers.
    Specific(Vec<u64>),
    /// Extract frames from multiple disjoint time segments.
    ///
    /// Each `(start, end)` pair defines a time range. Segments may be
    /// non-contiguous and may appear in any order; they are sorted and
    /// merged internally.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use unbundle::{FrameRange, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let frames = unbundler.video().frames(FrameRange::Segments(vec![
    ///     (Duration::from_secs(0), Duration::from_secs(2)),
    ///     (Duration::from_secs(10), Duration::from_secs(12)),
    /// ]))?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    Segments(Vec<(Duration, Duration)>),
}

/// Video frame extraction operations.
///
/// Obtained via [`MediaFile::video`] or
/// [`MediaFile::video_track`]. Each extraction method creates a
/// fresh decoder, seeks to the relevant position, and decodes frames. The
/// decoder is dropped when the method returns.
///
/// Frames are returned as [`DynamicImage`], with pixel format controlled
/// by [`ExtractOptions`].
pub struct VideoHandle<'a> {
    pub(crate) unbundler: &'a mut MediaFile,
    /// Optional stream index override for multi-track selection.
    pub(crate) stream_index: Option<usize>,
    /// Cached decoder/scaler state reused across consecutive
    /// [`frame_with_options`](VideoHandle::frame_with_options) calls.
    pub(crate) cached: Option<CachedDecoderState>,
}

/// Decoder and scaler state cached between consecutive single-frame
/// extractions via [`VideoHandle::frame_with_options`].
///
/// Keeping the decoder alive avoids the overhead of re-creating the
/// codec context and scaler on every call.  The cache is invalidated
/// when the output configuration (dimensions, pixel format) changes.
pub(crate) struct CachedDecoderState {
    decoder: VideoDecoder,
    scaler: ScalingContext,
    time_base: Rational,
    output_pixel: Pixel,
    target_width: u32,
    target_height: u32,
    decoded_frame: VideoFrame,
    scaled_frame: VideoFrame,
    /// PTS of the last frame handed back to the caller.
    last_pts: Option<i64>,
    /// `true` after `send_eof()` has been called on the decoder.
    eof_sent: bool,
}

impl<'a> VideoHandle<'a> {
    /// Resolve the video stream index to use.
    fn resolve_video_stream_index(&self) -> Result<usize, UnbundleError> {
        self.stream_index
            .or(self.unbundler.video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)
    }
    /// Extract a single frame by frame number (0-indexed).
    ///
    /// Seeks to the nearest keyframe before the target and decodes forward
    /// until the requested frame is reached. Uses default output settings
    /// (RGB8, source resolution).
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoVideoStream`] if the file has no video.
    /// - [`UnbundleError::FrameOutOfRange`] if `frame_number` exceeds the
    ///   frame count.
    /// - [`UnbundleError::VideoDecodeError`] if decoding fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let frame = unbundler.video().frame(100)?;
    /// frame.save("frame_100.png")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frame(&mut self, frame_number: u64) -> Result<DynamicImage, UnbundleError> {
        self.frame_with_options(frame_number, &ExtractOptions::default())
    }

    /// Extract a single frame with custom configuration.
    ///
    /// Like [`frame`](VideoHandle::frame) but respects the pixel format,
    /// resolution, and hardware acceleration settings from the given
    /// [`ExtractOptions`].
    ///
    /// # Errors
    ///
    /// Same as [`frame`](VideoHandle::frame), plus
    /// [`UnbundleError::Cancelled`] if cancellation is requested.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{ExtractOptions, MediaFile, PixelFormat, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let config = ExtractOptions::new()
    ///     .with_pixel_format(PixelFormat::Gray8)
    ///     .with_resolution(Some(320), None);
    /// let frame = unbundler.video().frame_with_options(100, &config)?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frame_with_options(
        &mut self,
        frame_number: u64,
        config: &ExtractOptions,
    ) -> Result<DynamicImage, UnbundleError> {
        let video_stream_index = self.resolve_video_stream_index()?;

        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?;

        let total_frames = video_metadata.frame_count;
        let frames_per_second = video_metadata.frames_per_second;
        let (target_width, target_height) = config
            .frame_output
            .resolve_dimensions(video_metadata.width, video_metadata.height);
        let output_pixel = config.frame_output.pixel_format.to_ffmpeg_pixel();

        if total_frames > 0 && frame_number >= total_frames {
            return Err(UnbundleError::FrameOutOfRange {
                frame_number,
                total_frames,
            });
        }

        if config.is_cancelled() {
            return Err(UnbundleError::Cancelled);
        }

        log::debug!(
            "Extracting frame {} (fps={:.2}, stream={})",
            frame_number,
            frames_per_second,
            video_stream_index
        );

        // ── Reuse or create decoder/scaler ──────────────────────────
        let need_new = match &self.cached {
            Some(c) => {
                c.target_width != target_width
                    || c.target_height != target_height
                    || c.output_pixel != output_pixel
            }
            None => true,
        };

        if need_new {
            let stream = self
                .unbundler
                .input_context
                .stream(video_stream_index)
                .ok_or(UnbundleError::NoVideoStream)?;
            let time_base = stream.time_base();
            let codec_parameters = stream.parameters();
            let decoder_context = CodecContext::from_parameters(codec_parameters)?;
            let decoder = decoder_context.decoder().video()?;

            let scaler = ScalingContext::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                output_pixel,
                target_width,
                target_height,
                ScalingFlags::BILINEAR,
            )?;

            self.cached = Some(CachedDecoderState {
                decoder,
                scaler,
                time_base,
                output_pixel,
                target_width,
                target_height,
                decoded_frame: VideoFrame::empty(),
                scaled_frame: VideoFrame::empty(),
                last_pts: None,
                eof_sent: false,
            });
        }

        // ── Decide: seek or decode forward ──────────────────────────
        let seek_timestamp =
            crate::conversion::frame_number_to_seek_timestamp(frame_number, frames_per_second);

        let state = self.cached.as_mut().unwrap();

        // Reset from EOF state if needed.
        if state.eof_sent {
            state.decoder.flush();
            state.eof_sent = false;
            state.last_pts = None;
        }

        let should_seek = match state.last_pts {
            None => true,
            Some(last) => {
                let target_pts_approx =
                    (frame_number as f64 / frames_per_second * state.time_base.denominator() as f64
                        / state.time_base.numerator().max(1) as f64) as i64;
                // Seek if target is before current position or >2 s ahead.
                target_pts_approx < last
                    || crate::conversion::pts_to_seconds(target_pts_approx - last, state.time_base)
                        > 2.0
            }
        };

        if should_seek {
            state.decoder.flush();
            state.last_pts = None;
            self.unbundler
                .input_context
                .seek(seek_timestamp, ..seek_timestamp)?;
        }

        // ── Try buffered frames first ───────────────────────────────
        {
            let state = self.cached.as_mut().unwrap();
            while state
                .decoder
                .receive_frame(&mut state.decoded_frame)
                .is_ok()
            {
                let pts = state.decoded_frame.pts().unwrap_or(0);
                state.last_pts = Some(pts);
                let current_frame_number =
                    crate::conversion::pts_to_frame_number(pts, state.time_base, frames_per_second);

                if current_frame_number >= frame_number {
                    state
                        .scaler
                        .run(&state.decoded_frame, &mut state.scaled_frame)?;
                    return convert_frame_to_image(
                        &state.scaled_frame,
                        state.target_width,
                        state.target_height,
                        &config.frame_output,
                    );
                }
            }
        }

        // ── Read new packets and decode ─────────────────────────────
        for (stream, packet) in self.unbundler.input_context.packets() {
            if stream.index() != video_stream_index {
                continue;
            }

            let state = self.cached.as_mut().unwrap();
            state.decoder.send_packet(&packet)?;

            while state
                .decoder
                .receive_frame(&mut state.decoded_frame)
                .is_ok()
            {
                let pts = state.decoded_frame.pts().unwrap_or(0);
                state.last_pts = Some(pts);
                let current_frame_number =
                    crate::conversion::pts_to_frame_number(pts, state.time_base, frames_per_second);

                if current_frame_number >= frame_number {
                    state
                        .scaler
                        .run(&state.decoded_frame, &mut state.scaled_frame)?;
                    return convert_frame_to_image(
                        &state.scaled_frame,
                        state.target_width,
                        state.target_height,
                        &config.frame_output,
                    );
                }
            }
        }

        // ── Flush the decoder ───────────────────────────────────────
        let state = self.cached.as_mut().unwrap();
        state.decoder.send_eof()?;
        state.eof_sent = true;

        while state
            .decoder
            .receive_frame(&mut state.decoded_frame)
            .is_ok()
        {
            let pts = state.decoded_frame.pts().unwrap_or(0);
            state.last_pts = Some(pts);
            let current_frame_number =
                crate::conversion::pts_to_frame_number(pts, state.time_base, frames_per_second);

            if current_frame_number >= frame_number {
                state
                    .scaler
                    .run(&state.decoded_frame, &mut state.scaled_frame)?;
                return convert_frame_to_image(
                    &state.scaled_frame,
                    state.target_width,
                    state.target_height,
                    &config.frame_output,
                );
            }
        }

        Err(UnbundleError::VideoDecodeError(format!(
            "Could not locate frame {frame_number} in the video stream"
        )))
    }

    /// Extract a single frame at a specific timestamp.
    ///
    /// Converts the timestamp to a frame number using the video's frame rate
    /// and delegates to [`frame`](VideoHandle::frame).
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::InvalidTimestamp`] if the timestamp exceeds the
    /// media duration, or any error from [`frame`](VideoHandle::frame).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    /// use std::time::Duration;
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let frame = unbundler.video().frame_at(Duration::from_secs(30))?;
    /// frame.save("frame_at_30s.png")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frame_at(&mut self, timestamp: Duration) -> Result<DynamicImage, UnbundleError> {
        self.frame_at_with_options(timestamp, &ExtractOptions::default())
    }

    /// Extract a single frame at a timestamp with custom configuration.
    ///
    /// Like [`frame_at`](VideoHandle::frame_at) but respects the pixel
    /// format, resolution, and hardware acceleration settings from the given
    /// [`ExtractOptions`].
    ///
    /// # Errors
    ///
    /// Same as [`frame_at`](VideoHandle::frame_at), plus
    /// [`UnbundleError::Cancelled`] if cancellation is requested.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use unbundle::{ExtractOptions, MediaFile, PixelFormat, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let config = ExtractOptions::new()
    ///     .with_pixel_format(PixelFormat::Rgba8);
    /// let frame = unbundler.video().frame_at_with_options(
    ///     Duration::from_secs(30),
    ///     &config,
    /// )?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frame_at_with_options(
        &mut self,
        timestamp: Duration,
        config: &ExtractOptions,
    ) -> Result<DynamicImage, UnbundleError> {
        let duration = self.unbundler.metadata.duration;
        if timestamp > duration {
            return Err(UnbundleError::InvalidTimestamp(timestamp));
        }

        let frames_per_second = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .frames_per_second;

        let frame_number =
            crate::conversion::timestamp_to_frame_number(timestamp, frames_per_second);
        self.frame_with_options(frame_number, config)
    }

    /// Extract a single frame by number, returning both the image and its
    /// [`FrameMetadata`] metadata.
    ///
    /// This combines frame extraction with metadata collection (PTS,
    /// keyframe flag, picture type) in a single decode pass.
    ///
    /// # Errors
    ///
    /// Same as [`frame`](VideoHandle::frame).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let (image, info) = unbundler.video().frame_and_metadata(42)?;
    /// println!("PTS: {:?}, keyframe: {}", info.pts, info.is_keyframe);
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frame_and_metadata(
        &mut self,
        frame_number: u64,
    ) -> Result<(DynamicImage, FrameMetadata), UnbundleError> {
        self.frame_and_metadata_with_options(frame_number, &ExtractOptions::default())
    }

    /// Extract a single frame with [`FrameMetadata`] and custom configuration.
    ///
    /// Like [`frame_and_metadata`](VideoHandle::frame_and_metadata) but respects
    /// the pixel format, resolution, and hardware acceleration settings from
    /// the given [`ExtractOptions`].
    ///
    /// # Errors
    ///
    /// Same as [`frame_and_metadata`](VideoHandle::frame_and_metadata), plus
    /// [`UnbundleError::Cancelled`] if cancellation is requested.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{ExtractOptions, MediaFile, PixelFormat, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let config = ExtractOptions::new()
    ///     .with_pixel_format(PixelFormat::Gray8);
    /// let (image, info) = unbundler.video().frame_and_metadata_with_options(42, &config)?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frame_and_metadata_with_options(
        &mut self,
        frame_number: u64,
        config: &ExtractOptions,
    ) -> Result<(DynamicImage, FrameMetadata), UnbundleError> {
        let video_stream_index = self.resolve_video_stream_index()?;

        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?;

        let total_frames = video_metadata.frame_count;
        let frames_per_second = video_metadata.frames_per_second;
        let (target_width, target_height) = config
            .frame_output
            .resolve_dimensions(video_metadata.width, video_metadata.height);
        let output_pixel = config.frame_output.pixel_format.to_ffmpeg_pixel();

        if total_frames > 0 && frame_number >= total_frames {
            return Err(UnbundleError::FrameOutOfRange {
                frame_number,
                total_frames,
            });
        }

        if config.is_cancelled() {
            return Err(UnbundleError::Cancelled);
        }

        let stream = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?;
        let time_base = stream.time_base();
        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let mut decoder = decoder_context.decoder().video()?;

        let mut scaler = ScalingContext::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            output_pixel,
            target_width,
            target_height,
            ScalingFlags::BILINEAR,
        )?;

        let seek_timestamp =
            crate::conversion::frame_number_to_seek_timestamp(frame_number, frames_per_second);

        self.unbundler
            .input_context
            .seek(seek_timestamp, ..seek_timestamp)?;

        let mut decoded_frame = VideoFrame::empty();
        let mut rgb_frame = VideoFrame::empty();

        for (stream, packet) in self.unbundler.input_context.packets() {
            if stream.index() != video_stream_index {
                continue;
            }

            decoder.send_packet(&packet)?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                let pts = decoded_frame.pts().unwrap_or(0);
                let current_frame_number =
                    crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

                if current_frame_number >= frame_number {
                    let info = build_frame_info(&decoded_frame, current_frame_number, time_base);
                    scaler.run(&decoded_frame, &mut rgb_frame)?;
                    let image = convert_frame_to_image(
                        &rgb_frame,
                        target_width,
                        target_height,
                        &config.frame_output,
                    )?;
                    return Ok((image, info));
                }
            }
        }

        decoder.send_eof()?;
        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            let pts = decoded_frame.pts().unwrap_or(0);
            let current_frame_number =
                crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

            if current_frame_number >= frame_number {
                let info = build_frame_info(&decoded_frame, current_frame_number, time_base);
                scaler.run(&decoded_frame, &mut rgb_frame)?;
                let image = convert_frame_to_image(
                    &rgb_frame,
                    target_width,
                    target_height,
                    &config.frame_output,
                )?;
                return Ok((image, info));
            }
        }

        Err(UnbundleError::VideoDecodeError(format!(
            "Could not locate frame {frame_number} in the video stream"
        )))
    }

    /// Extract multiple frames with their [`FrameMetadata`] metadata.
    ///
    /// Like [`frames`](VideoHandle::frames) but returns
    /// `(DynamicImage, FrameMetadata)` pairs, giving access to PTS, keyframe
    /// flags, and picture types for every extracted frame.
    ///
    /// # Errors
    ///
    /// Same as [`frames`](VideoHandle::frames).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{FrameRange, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let results = unbundler.video().frames_and_metadata(FrameRange::Range(0, 9))?;
    /// for (image, info) in &results {
    ///     println!("Frame {} — type {:?}", info.frame_number, info.frame_type);
    /// }
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frames_and_metadata(
        &mut self,
        range: FrameRange,
    ) -> Result<Vec<(DynamicImage, FrameMetadata)>, UnbundleError> {
        self.frames_and_metadata_with_options(range, &ExtractOptions::default())
    }

    /// Extract multiple frames with [`FrameMetadata`] and progress/cancellation.
    ///
    /// Like [`frames_with_options`](VideoHandle::frames_with_options) but
    /// includes [`FrameMetadata`] for each frame.
    ///
    /// # Errors
    ///
    /// Same as [`frames_with_options`](VideoHandle::frames_with_options).
    pub fn frames_and_metadata_with_options(
        &mut self,
        range: FrameRange,
        config: &ExtractOptions,
    ) -> Result<Vec<(DynamicImage, FrameMetadata)>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let total =
            Self::estimate_frame_count(&range, &video_metadata, self.unbundler.metadata.duration);

        let mut tracker = ProgressTracker::new(
            config.progress.clone(),
            OperationType::FrameExtraction,
            total,
            config.batch_size,
        );

        let mut results = Vec::with_capacity(total.unwrap_or(0) as usize);

        self.dispatch_range_with_info(
            range,
            &video_metadata,
            config,
            &mut |_frame_number, img, info| {
                results.push((img, info));
                tracker.advance(Some(_frame_number), None);
                Ok(())
            },
        )?;

        tracker.finish();
        Ok(results)
    }

    /// Extract a frame and save it directly to a file.
    ///
    /// Convenience method that combines [`frame`](VideoHandle::frame) with
    /// [`DynamicImage::save`]. The output format is inferred from the file
    /// extension.
    ///
    /// # Errors
    ///
    /// Returns errors from [`frame`](VideoHandle::frame), or
    /// [`UnbundleError::ImageError`] if the image cannot be written.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// unbundler.video().save_frame(0, "first_frame.png")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn save_frame<P: AsRef<Path>>(
        &mut self,
        frame_number: u64,
        path: P,
    ) -> Result<(), UnbundleError> {
        let image = self.frame(frame_number)?;
        image.save(path)?;
        Ok(())
    }

    /// Extract a frame at a timestamp and save it directly to a file.
    ///
    /// Convenience method that combines [`frame_at`](VideoHandle::frame_at)
    /// with [`DynamicImage::save`]. The output format is inferred from the file
    /// extension.
    ///
    /// # Errors
    ///
    /// Returns errors from [`frame_at`](VideoHandle::frame_at), or
    /// [`UnbundleError::ImageError`] if the image cannot be written.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// unbundler.video().save_frame_at(Duration::from_secs(5), "frame_5s.png")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn save_frame_at<P: AsRef<Path>>(
        &mut self,
        timestamp: Duration,
        path: P,
    ) -> Result<(), UnbundleError> {
        let image = self.frame_at(timestamp)?;
        image.save(path)?;
        Ok(())
    }

    /// Extract multiple frames according to the specified range.
    ///
    /// See [`FrameRange`] for the available selection modes.
    ///
    /// # Errors
    ///
    /// Returns errors from individual frame extraction, or
    /// [`UnbundleError::NoVideoStream`] if the file has no video.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{FrameRange, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let frames = unbundler.video().frames(FrameRange::Range(0, 9))?;
    /// assert_eq!(frames.len(), 10);
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frames(&mut self, range: FrameRange) -> Result<Vec<DynamicImage>, UnbundleError> {
        self.frames_with_options(range, &ExtractOptions::default())
    }

    /// Process frames one at a time without collecting them into a `Vec`.
    ///
    /// This is a streaming alternative to [`frames`](VideoHandle::frames)
    /// that calls `callback` for each decoded frame. The callback receives the
    /// frame number and the decoded image. Processing stops if the callback
    /// returns an error.
    ///
    /// # Errors
    ///
    /// Returns the first error from decoding or from the callback.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{FrameRange, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// unbundler.video().for_each_frame(
    ///     FrameRange::Range(0, 9),
    ///     |frame_number, image| {
    ///         image.save(format!("frame_{frame_number}.png"))?;
    ///         Ok(())
    ///     },
    /// )?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn for_each_frame<F>(&mut self, range: FrameRange, callback: F) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage) -> Result<(), UnbundleError>,
    {
        self.for_each_frame_with_options(range, &ExtractOptions::default(), callback)
    }

    /// Extract multiple frames with progress reporting and cancellation.
    ///
    /// Like [`frames`](VideoHandle::frames) but accepts an
    /// [`ExtractOptions`] for progress callbacks and cancellation support.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested,
    /// or any error from [`frames`](VideoHandle::frames).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::sync::Arc;
    ///
    /// use unbundle::{ExtractOptions, FrameRange, MediaFile, ProgressCallback, ProgressInfo, UnbundleError};
    ///
    /// struct PrintProgress;
    /// impl ProgressCallback for PrintProgress {
    ///     fn on_progress(&self, info: &ProgressInfo) {
    ///         println!("Frame {}/{}", info.current, info.total.unwrap_or(0));
    ///     }
    /// }
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let config = ExtractOptions::new()
    ///     .with_progress(Arc::new(PrintProgress));
    /// let frames = unbundler.video().frames_with_options(
    ///     FrameRange::Range(0, 9),
    ///     &config,
    /// )?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frames_with_options(
        &mut self,
        range: FrameRange,
        config: &ExtractOptions,
    ) -> Result<Vec<DynamicImage>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let total =
            Self::estimate_frame_count(&range, &video_metadata, self.unbundler.metadata.duration);

        let mut tracker = ProgressTracker::new(
            config.progress.clone(),
            OperationType::FrameExtraction,
            total,
            config.batch_size,
        );

        let mut frames = Vec::with_capacity(total.unwrap_or(0) as usize);

        self.dispatch_range(range, &video_metadata, config, &mut |frame_number, img| {
            frames.push(img);
            tracker.advance(Some(frame_number), None);
            Ok(())
        })?;

        tracker.finish();
        Ok(frames)
    }

    /// Process frames one at a time with progress reporting and cancellation.
    ///
    /// Like [`for_each_frame`](VideoHandle::for_each_frame) but accepts an
    /// [`ExtractOptions`].
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested,
    /// or any error from decoding or the callback.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{CancellationToken, ExtractOptions, FrameRange, MediaFile, UnbundleError};
    ///
    /// let token = CancellationToken::new();
    /// let config = ExtractOptions::new()
    ///     .with_cancellation(token.clone());
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// unbundler.video().for_each_frame_with_options(
    ///     FrameRange::Range(0, 99),
    ///     &config,
    ///     |frame_number, image| {
    ///         image.save(format!("frame_{frame_number}.png"))?;
    ///         Ok(())
    ///     },
    /// )?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn for_each_frame_with_options<F>(
        &mut self,
        range: FrameRange,
        config: &ExtractOptions,
        mut callback: F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage) -> Result<(), UnbundleError>,
    {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let total =
            Self::estimate_frame_count(&range, &video_metadata, self.unbundler.metadata.duration);

        let mut tracker = ProgressTracker::new(
            config.progress.clone(),
            OperationType::FrameExtraction,
            total,
            config.batch_size,
        );

        self.dispatch_range(range, &video_metadata, config, &mut |frame_number, img| {
            callback(frame_number, img)?;
            tracker.advance(Some(frame_number), None);
            Ok(())
        })?;

        tracker.finish();
        Ok(())
    }

    /// Detect scene changes (shot boundaries) in the video.
    ///
    /// Uses FFmpeg's `scdet` filter to analyse every frame and return a list
    /// of [`SceneChange`](crate::scene::SceneChange) entries.
    ///
    /// An optional [`SceneDetectionOptions`](crate::scene::SceneDetectionOptions)
    /// controls the detection threshold. Pass `None` for defaults (threshold
    /// 10.0).
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoVideoStream`] if the file has no video.
    /// - [`UnbundleError::VideoDecodeError`] if the `scdet` filter is not
    ///   available in your FFmpeg build.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let scenes = unbundler.video().detect_scenes(None)?;
    /// println!("Found {} scene changes", scenes.len());
    /// # Ok::<(), UnbundleError>(())
    /// ```
    #[cfg(feature = "scene")]
    pub fn detect_scenes(
        &mut self,
        config: Option<SceneDetectionOptions>,
    ) -> Result<Vec<SceneChange>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let scd_config = config.unwrap_or_default();
        crate::scene::detect_scenes_impl(
            self.unbundler,
            &video_metadata,
            &scd_config,
            None,
            self.stream_index,
        )
    }

    /// Detect scene changes with cancellation support.
    ///
    /// Like [`detect_scenes`](VideoHandle::detect_scenes) but accepts an
    /// [`ExtractOptions`] for cancellation.
    #[cfg(feature = "scene")]
    pub fn detect_scenes_with_options(
        &mut self,
        scd_config: Option<SceneDetectionOptions>,
        config: &ExtractOptions,
    ) -> Result<Vec<SceneChange>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let scd_config = scd_config.unwrap_or_default();
        let cancel_check: Box<dyn Fn() -> bool> = Box::new(|| config.is_cancelled());
        crate::scene::detect_scenes_impl(
            self.unbundler,
            &video_metadata,
            &scd_config,
            Some(&*cancel_check),
            self.stream_index,
        )
    }

    /// Export frames as an animated GIF to a file.
    ///
    /// Extracts frames matching the given [`FrameRange`], scales them
    /// according to [`GifOptions`], and writes the result as an animated
    /// GIF.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoVideoStream`] if no video stream exists.
    /// - [`UnbundleError::GifEncodeError`] if encoding fails.
    /// - Any errors from frame extraction.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use unbundle::{FrameRange, GifOptions, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let config = GifOptions::new().width(320).frame_delay(10);
    /// unbundler.video().export_gif(
    ///     "output.gif",
    ///     FrameRange::TimeRange(Duration::from_secs(0), Duration::from_secs(5)),
    ///     &config,
    /// )?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    #[cfg(feature = "gif")]
    pub fn export_gif<P: AsRef<Path>>(
        &mut self,
        path: P,
        range: FrameRange,
        gif_config: &GifOptions,
    ) -> Result<(), UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let foc = gif_config.to_frame_output_config(video_metadata.width, video_metadata.height);
        let extraction_config = ExtractOptions::default().with_frame_output(foc);
        let frames = self.frames_with_options(range, &extraction_config)?;
        crate::gif::encode_gif(path, &frames, gif_config)
    }

    /// Export frames as an animated GIF into memory.
    ///
    /// Like [`export_gif`](VideoHandle::export_gif) but returns the
    /// raw GIF bytes instead of writing to a file.
    ///
    /// # Errors
    ///
    /// Same as [`export_gif`](VideoHandle::export_gif).
    #[cfg(feature = "gif")]
    pub fn export_gif_to_memory(
        &mut self,
        range: FrameRange,
        gif_config: &GifOptions,
    ) -> Result<Vec<u8>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let foc = gif_config.to_frame_output_config(video_metadata.width, video_metadata.height);
        let extraction_config = ExtractOptions::default().with_frame_output(foc);
        let frames = self.frames_with_options(range, &extraction_config)?;
        crate::gif::encode_gif_to_memory(&frames, gif_config)
    }

    /// Analyze the Group of Pictures structure of the video stream.
    ///
    /// Scans all video packets (without decoding) to identify keyframes and
    /// compute Group of Pictures statistics such as average, minimum and maximum sequence size.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoVideoStream`] if no video stream exists.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let group_of_pictures = unbundler.video().analyze_group_of_pictures()?;
    /// println!(
    ///     "Keyframes: {}, Average Group of Pictures: {:.1}",
    ///     group_of_pictures.keyframes.len(),
    ///     group_of_pictures.average_group_of_pictures_size
    /// );
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn analyze_group_of_pictures(&mut self) -> Result<GroupOfPicturesInfo, UnbundleError> {
        let video_stream_index = self.resolve_video_stream_index()?;
        crate::keyframe::analyze_group_of_pictures_impl(self.unbundler, video_stream_index)
    }

    /// Return a list of all keyframes in the video stream.
    ///
    /// This is a convenience wrapper around
    /// [`analyze_group_of_pictures`](VideoHandle::analyze_group_of_pictures)
    /// that returns only the keyframe list.
    ///
    /// # Errors
    ///
    /// Same as [`analyze_group_of_pictures`](VideoHandle::analyze_group_of_pictures).
    pub fn keyframes(&mut self) -> Result<Vec<KeyFrameMetadata>, UnbundleError> {
        Ok(self.analyze_group_of_pictures()?.keyframes)
    }

    /// Analyze the video stream for variable frame rate (VFR).
    ///
    /// Scans all video packet PTS values and computes timing statistics.
    /// The result indicates whether the stream is VFR, plus min/max/mean
    /// FPS and per-frame PTS values.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoVideoStream`] if no video stream exists.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let vfr = unbundler.video().analyze_variable_framerate()?;
    /// println!("VFR: {}, mean FPS: {:.2}", vfr.is_vfr, vfr.mean_fps);
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn analyze_variable_framerate(
        &mut self,
    ) -> Result<VariableFrameRateAnalysis, UnbundleError> {
        let video_stream_index = self.resolve_video_stream_index()?;
        crate::variable_framerate::analyze_variable_framerate_impl(
            self.unbundler,
            video_stream_index,
        )
    }

    /// Create an async stream of decoded video frames.
    ///
    /// Returns a [`FrameStream`] that
    /// yields `(frame_number, DynamicImage)` pairs from a background
    /// blocking thread. The stream implements
    /// [`tokio_stream::Stream`] and can be used with `StreamExt`
    /// combinators.
    ///
    /// A fresh demuxer is opened internally so this method returns
    /// immediately and the mutable borrow on the unbundler is released.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::NoVideoStream`] if the file has no video
    /// stream (validated eagerly before spawning the background thread).
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
    ///     .frame_stream(FrameRange::Range(0, 9), ExtractOptions::new())?;
    ///
    /// while let Some(result) = stream.next().await {
    ///     let (frame_number, image) = result?;
    ///     image.save(format!("frame_{frame_number}.png"))?;
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "async")]
    pub fn frame_stream(
        &mut self,
        range: FrameRange,
        config: ExtractOptions,
    ) -> Result<FrameStream, UnbundleError> {
        // Validate eagerly: ensure the file has a video stream.
        let _video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?;

        let file_path = self.unbundler.file_path.clone();
        Ok(crate::stream::create_frame_stream(
            file_path, range, config, None,
        ))
    }

    /// Create a lazy iterator over decoded video frames.
    ///
    /// Unlike [`frames`](VideoHandle::frames), which decodes everything
    /// up-front, this returns a [`FrameIterator`]
    /// that decodes one frame at a time on each [`next()`](Iterator::next)
    /// call. This is ideal when you want to stop early or process frames
    /// one by one without buffering the entire set.
    ///
    /// The iterator borrows the underlying [`MediaFile`] mutably and
    /// releases it when dropped.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::NoVideoStream`] if the file has no video,
    /// or validation errors from the range.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{FrameRange, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let iter = unbundler.video().frame_iter(FrameRange::Range(0, 9))?;
    ///
    /// for result in iter {
    ///     let (frame_number, image) = result?;
    ///     image.save(format!("frame_{frame_number}.png"))?;
    /// }
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frame_iter(self, range: FrameRange) -> Result<FrameIterator<'a>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let frame_numbers = self.resolve_frame_numbers_for_iter(range, &video_metadata)?;
        let output_config = FrameOutputOptions::default();

        FrameIterator::new(
            self.unbundler,
            frame_numbers,
            output_config,
            self.stream_index,
        )
    }

    /// Create a lazy iterator with custom output configuration.
    ///
    /// Like [`frame_iter`](VideoHandle::frame_iter) but uses the given
    /// [`FrameOutputOptions`] for pixel format and resolution settings.
    ///
    /// # Errors
    ///
    /// Returns errors from [`frame_iter`](VideoHandle::frame_iter).
    pub fn frame_iter_with_options(
        self,
        range: FrameRange,
        output_config: FrameOutputOptions,
    ) -> Result<FrameIterator<'a>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let frame_numbers = self.resolve_frame_numbers_for_iter(range, &video_metadata)?;
        FrameIterator::new(
            self.unbundler,
            frame_numbers,
            output_config,
            self.stream_index,
        )
    }

    /// Resolve a [`FrameRange`] into sorted, deduplicated frame numbers.
    ///
    /// Shared helper for [`frame_iter`](VideoHandle::frame_iter) and
    /// related methods.
    fn resolve_frame_numbers_for_iter(
        &self,
        range: FrameRange,
        video_metadata: &VideoMetadata,
    ) -> Result<Vec<u64>, UnbundleError> {
        let mut numbers = match range {
            FrameRange::Range(start, end) => {
                if start > end {
                    return Err(UnbundleError::InvalidRange {
                        start: format!("frame {start}"),
                        end: format!("frame {end}"),
                    });
                }
                (start..=end).collect()
            }
            FrameRange::Interval(step) => {
                if step == 0 {
                    return Err(UnbundleError::InvalidInterval);
                }
                (0..video_metadata.frame_count)
                    .step_by(step as usize)
                    .collect()
            }
            FrameRange::TimeRange(start_time, end_time) => {
                if start_time >= end_time {
                    return Err(UnbundleError::InvalidRange {
                        start: format!("{start_time:?}"),
                        end: format!("{end_time:?}"),
                    });
                }
                let start_frame = crate::conversion::timestamp_to_frame_number(
                    start_time,
                    video_metadata.frames_per_second,
                );
                let end_frame = crate::conversion::timestamp_to_frame_number(
                    end_time,
                    video_metadata.frames_per_second,
                );
                (start_frame..=end_frame).collect()
            }
            FrameRange::TimeInterval(interval) => {
                if interval.is_zero() {
                    return Err(UnbundleError::InvalidInterval);
                }
                let total_duration = self.unbundler.metadata.duration;
                let mut nums = Vec::new();
                let mut current = Duration::ZERO;
                while current <= total_duration {
                    nums.push(crate::conversion::timestamp_to_frame_number(
                        current,
                        video_metadata.frames_per_second,
                    ));
                    current += interval;
                }
                nums
            }
            FrameRange::Specific(nums) => nums,
            FrameRange::Segments(segments) => Self::resolve_segments(&segments, video_metadata)?,
        };
        numbers.sort_unstable();
        numbers.dedup();
        Ok(numbers)
    }

    /// Resolve a list of `(start, end)` time segments into sorted,
    /// deduplicated frame numbers.
    fn resolve_segments(
        segments: &[(Duration, Duration)],
        video_metadata: &VideoMetadata,
    ) -> Result<Vec<u64>, UnbundleError> {
        let mut numbers = Vec::new();
        for (start, end) in segments {
            if start >= end {
                return Err(UnbundleError::InvalidRange {
                    start: format!("{start:?}"),
                    end: format!("{end:?}"),
                });
            }
            let start_frame = crate::conversion::timestamp_to_frame_number(
                *start,
                video_metadata.frames_per_second,
            );
            let end_frame = crate::conversion::timestamp_to_frame_number(
                *end,
                video_metadata.frames_per_second,
            );
            numbers.extend(start_frame..=end_frame);
        }
        numbers.sort_unstable();
        numbers.dedup();
        Ok(numbers)
    }

    /// Extract multiple frames in parallel using rayon.
    ///
    /// Splits the requested frames across worker threads, each with its own
    /// demuxer and decoder. Returns frames sorted by frame number.
    ///
    /// This is most effective for large frame sets where frames are spread
    /// across the video (e.g. `FrameRange::Interval` or `FrameRange::Specific`
    /// with widely spaced numbers). For small ranges, sequential extraction is
    /// often faster due to per-thread file-open overhead.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::NoVideoStream`] if the file has no video
    /// stream, or errors from individual worker threads.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{ExtractOptions, FrameRange, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let config = ExtractOptions::new();
    /// let frames = unbundler
    ///     .video()
    ///     .frames_parallel(FrameRange::Interval(100), &config)?;
    /// println!("Got {} frames", frames.len());
    /// # Ok::<(), UnbundleError>(())
    /// ```
    #[cfg(feature = "rayon")]
    pub fn frames_parallel(
        &mut self,
        range: FrameRange,
        config: &ExtractOptions,
    ) -> Result<Vec<DynamicImage>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        // Resolve the range into concrete frame numbers.
        let frame_numbers = self.resolve_frame_numbers_for_iter(range, &video_metadata)?;

        let results = crate::rayon::parallel_extract_frames(
            &self.unbundler.file_path,
            &frame_numbers,
            &video_metadata,
            config,
        )?;

        Ok(results.into_iter().map(|(_, img)| img).collect())
    }

    /// Estimate the total number of frames a [`FrameRange`] will produce.
    fn estimate_frame_count(
        range: &FrameRange,
        video_metadata: &VideoMetadata,
        total_duration: Duration,
    ) -> Option<u64> {
        match range {
            FrameRange::Range(start, end) => {
                if end >= start {
                    Some(end - start + 1)
                } else {
                    Some(0)
                }
            }
            FrameRange::Interval(step) => {
                if *step == 0 {
                    return None;
                }
                Some((video_metadata.frame_count + step - 1) / step)
            }
            FrameRange::TimeRange(start_time, end_time) => {
                let fps = video_metadata.frames_per_second;
                let start_frame = crate::conversion::timestamp_to_frame_number(*start_time, fps);
                let end_frame = crate::conversion::timestamp_to_frame_number(*end_time, fps);
                Some(end_frame.saturating_sub(start_frame) + 1)
            }
            FrameRange::TimeInterval(interval) => {
                if interval.is_zero() {
                    return None;
                }
                let total_secs = total_duration.as_secs_f64();
                let interval_secs = interval.as_secs_f64();
                Some((total_secs / interval_secs).ceil() as u64 + 1)
            }
            FrameRange::Specific(numbers) => Some(numbers.len() as u64),
            FrameRange::Segments(segments) => {
                let fps = video_metadata.frames_per_second;
                let total: u64 = segments
                    .iter()
                    .map(|(start, end)| {
                        let sf = crate::conversion::timestamp_to_frame_number(*start, fps);
                        let ef = crate::conversion::timestamp_to_frame_number(*end, fps);
                        ef.saturating_sub(sf) + 1
                    })
                    .sum();
                Some(total)
            }
        }
    }

    /// Validate and dispatch a [`FrameRange`] to the appropriate processing
    /// method, with cancellation support.
    fn dispatch_range<F>(
        &mut self,
        range: FrameRange,
        video_metadata: &VideoMetadata,
        config: &ExtractOptions,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage) -> Result<(), UnbundleError>,
    {
        match range {
            FrameRange::Range(start, end) => {
                if start > end {
                    return Err(UnbundleError::InvalidRange {
                        start: format!("frame {start}"),
                        end: format!("frame {end}"),
                    });
                }
                self.process_frame_range(start, end, video_metadata, config, handler)
            }
            FrameRange::Interval(step) => {
                if step == 0 {
                    return Err(UnbundleError::InvalidInterval);
                }
                let total = video_metadata.frame_count;
                let numbers: Vec<u64> = (0..total).step_by(step as usize).collect();
                self.process_specific_frames(&numbers, video_metadata, config, handler)
            }
            FrameRange::TimeRange(start_time, end_time) => {
                if start_time >= end_time {
                    return Err(UnbundleError::InvalidRange {
                        start: format!("{start_time:?}"),
                        end: format!("{end_time:?}"),
                    });
                }
                let start_frame = crate::conversion::timestamp_to_frame_number(
                    start_time,
                    video_metadata.frames_per_second,
                );
                let end_frame = crate::conversion::timestamp_to_frame_number(
                    end_time,
                    video_metadata.frames_per_second,
                );
                self.process_frame_range(start_frame, end_frame, video_metadata, config, handler)
            }
            FrameRange::TimeInterval(interval) => {
                if interval.is_zero() {
                    return Err(UnbundleError::InvalidInterval);
                }
                let total_duration = self.unbundler.metadata.duration;
                let mut numbers = Vec::new();
                let mut current = Duration::ZERO;
                while current <= total_duration {
                    numbers.push(crate::conversion::timestamp_to_frame_number(
                        current,
                        video_metadata.frames_per_second,
                    ));
                    current += interval;
                }
                self.process_specific_frames(&numbers, video_metadata, config, handler)
            }
            FrameRange::Specific(numbers) => {
                self.process_specific_frames(&numbers, video_metadata, config, handler)
            }
            FrameRange::Segments(segments) => {
                let numbers = Self::resolve_segments(&segments, video_metadata)?;
                self.process_specific_frames(&numbers, video_metadata, config, handler)
            }
        }
    }

    /// Validate and dispatch a [`FrameRange`], passing [`FrameMetadata`]
    /// alongside each decoded image.
    fn dispatch_range_with_info<F>(
        &mut self,
        range: FrameRange,
        video_metadata: &VideoMetadata,
        config: &ExtractOptions,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage, FrameMetadata) -> Result<(), UnbundleError>,
    {
        match range {
            FrameRange::Range(start, end) => {
                if start > end {
                    return Err(UnbundleError::InvalidRange {
                        start: format!("frame {start}"),
                        end: format!("frame {end}"),
                    });
                }
                self.process_frame_range_with_info(start, end, video_metadata, config, handler)
            }
            FrameRange::Interval(step) => {
                if step == 0 {
                    return Err(UnbundleError::InvalidInterval);
                }
                let total = video_metadata.frame_count;
                let numbers: Vec<u64> = (0..total).step_by(step as usize).collect();
                self.process_specific_frames_and_metadata(&numbers, video_metadata, config, handler)
            }
            FrameRange::TimeRange(start_time, end_time) => {
                if start_time >= end_time {
                    return Err(UnbundleError::InvalidRange {
                        start: format!("{start_time:?}"),
                        end: format!("{end_time:?}"),
                    });
                }
                let start_frame = crate::conversion::timestamp_to_frame_number(
                    start_time,
                    video_metadata.frames_per_second,
                );
                let end_frame = crate::conversion::timestamp_to_frame_number(
                    end_time,
                    video_metadata.frames_per_second,
                );
                self.process_frame_range_with_info(
                    start_frame,
                    end_frame,
                    video_metadata,
                    config,
                    handler,
                )
            }
            FrameRange::TimeInterval(interval) => {
                if interval.is_zero() {
                    return Err(UnbundleError::InvalidInterval);
                }
                let total_duration = self.unbundler.metadata.duration;
                let mut numbers = Vec::new();
                let mut current = Duration::ZERO;
                while current <= total_duration {
                    numbers.push(crate::conversion::timestamp_to_frame_number(
                        current,
                        video_metadata.frames_per_second,
                    ));
                    current += interval;
                }
                self.process_specific_frames_and_metadata(&numbers, video_metadata, config, handler)
            }
            FrameRange::Specific(numbers) => {
                self.process_specific_frames_and_metadata(&numbers, video_metadata, config, handler)
            }
            FrameRange::Segments(segments) => {
                let numbers = Self::resolve_segments(&segments, video_metadata)?;
                self.process_specific_frames_and_metadata(&numbers, video_metadata, config, handler)
            }
        }
    }

    /// Decode a contiguous range of frames, calling the handler with
    /// [`FrameMetadata`] for each.
    fn process_frame_range_with_info<F>(
        &mut self,
        start: u64,
        end: u64,
        video_metadata: &VideoMetadata,
        config: &ExtractOptions,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage, FrameMetadata) -> Result<(), UnbundleError>,
    {
        let video_stream_index = self.resolve_video_stream_index()?;

        let (target_width, target_height) = config
            .frame_output
            .resolve_dimensions(video_metadata.width, video_metadata.height);
        let output_pixel = config.frame_output.pixel_format.to_ffmpeg_pixel();
        let frames_per_second = video_metadata.frames_per_second;

        let stream = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?;
        let time_base = stream.time_base();
        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let (mut decoder, hw_active) = create_video_decoder(decoder_context, config)?;

        let mut scaler: Option<ScalingContext> = if hw_active {
            None
        } else {
            Some(ScalingContext::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                output_pixel,
                target_width,
                target_height,
                ScalingFlags::BILINEAR,
            )?)
        };

        let seek_timestamp =
            crate::conversion::frame_number_to_seek_timestamp(start, frames_per_second);
        self.unbundler
            .input_context
            .seek(seek_timestamp, ..seek_timestamp)?;

        let mut decoded_frame = VideoFrame::empty();
        let mut scaled_frame = VideoFrame::empty();

        for (stream, packet) in self.unbundler.input_context.packets() {
            if config.is_cancelled() {
                return Err(UnbundleError::Cancelled);
            }
            if stream.index() != video_stream_index {
                continue;
            }

            decoder.send_packet(&packet)?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                let pts = decoded_frame.pts().unwrap_or(0);
                let current_frame_number =
                    crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

                if current_frame_number >= start && current_frame_number <= end {
                    let info = build_frame_info(&decoded_frame, current_frame_number, time_base);
                    let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    ensure_scaler(
                        &mut scaler,
                        source,
                        output_pixel,
                        target_width,
                        target_height,
                    )?;
                    scaler.as_mut().unwrap().run(source, &mut scaled_frame)?;
                    let image = convert_frame_to_image(
                        &scaled_frame,
                        target_width,
                        target_height,
                        &config.frame_output,
                    )?;
                    handler(current_frame_number, image, info)?;
                }

                if current_frame_number > end {
                    return Ok(());
                }
            }
        }

        decoder.send_eof()?;
        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            if config.is_cancelled() {
                return Err(UnbundleError::Cancelled);
            }
            let pts = decoded_frame.pts().unwrap_or(0);
            let current_frame_number =
                crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

            if current_frame_number >= start && current_frame_number <= end {
                let info = build_frame_info(&decoded_frame, current_frame_number, time_base);
                let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                let source = transferred.as_ref().unwrap_or(&decoded_frame);
                ensure_scaler(
                    &mut scaler,
                    source,
                    output_pixel,
                    target_width,
                    target_height,
                )?;
                scaler.as_mut().unwrap().run(source, &mut scaled_frame)?;
                let image = convert_frame_to_image(
                    &scaled_frame,
                    target_width,
                    target_height,
                    &config.frame_output,
                )?;
                handler(current_frame_number, image, info)?;
            }

            if current_frame_number > end {
                break;
            }
        }

        Ok(())
    }

    /// Process frames at specific (possibly non-contiguous) frame numbers,
    /// passing [`FrameMetadata`] alongside each decoded image.
    fn process_specific_frames_and_metadata<F>(
        &mut self,
        frame_numbers: &[u64],
        video_metadata: &VideoMetadata,
        config: &ExtractOptions,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage, FrameMetadata) -> Result<(), UnbundleError>,
    {
        if frame_numbers.is_empty() {
            return Ok(());
        }

        let video_stream_index = self.resolve_video_stream_index()?;

        let (target_width, target_height) = config
            .frame_output
            .resolve_dimensions(video_metadata.width, video_metadata.height);
        let output_pixel = config.frame_output.pixel_format.to_ffmpeg_pixel();
        let frames_per_second = video_metadata.frames_per_second;

        let mut sorted_numbers = frame_numbers.to_vec();
        sorted_numbers.sort_unstable();
        sorted_numbers.dedup();

        let stream = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?;
        let time_base = stream.time_base();
        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let (mut decoder, hw_active) = create_video_decoder(decoder_context, config)?;

        let mut scaler: Option<ScalingContext> = if hw_active {
            None
        } else {
            Some(ScalingContext::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                output_pixel,
                target_width,
                target_height,
                ScalingFlags::BILINEAR,
            )?)
        };

        let seek_timestamp =
            crate::conversion::frame_number_to_seek_timestamp(sorted_numbers[0], frames_per_second);
        self.unbundler
            .input_context
            .seek(seek_timestamp, ..seek_timestamp)?;

        let mut target_index = 0;
        let mut decoded_frame = VideoFrame::empty();
        let mut scaled_frame = VideoFrame::empty();

        for (stream, packet) in self.unbundler.input_context.packets() {
            if target_index >= sorted_numbers.len() {
                break;
            }
            if config.is_cancelled() {
                return Err(UnbundleError::Cancelled);
            }
            if stream.index() != video_stream_index {
                continue;
            }

            decoder.send_packet(&packet)?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                if target_index >= sorted_numbers.len() {
                    break;
                }

                let pts = decoded_frame.pts().unwrap_or(0);
                let current_frame_number =
                    crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

                while target_index < sorted_numbers.len()
                    && sorted_numbers[target_index] < current_frame_number
                {
                    target_index += 1;
                }

                if target_index < sorted_numbers.len()
                    && current_frame_number == sorted_numbers[target_index]
                {
                    let info = build_frame_info(&decoded_frame, current_frame_number, time_base);
                    let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    ensure_scaler(
                        &mut scaler,
                        source,
                        output_pixel,
                        target_width,
                        target_height,
                    )?;
                    scaler.as_mut().unwrap().run(source, &mut scaled_frame)?;
                    let image = convert_frame_to_image(
                        &scaled_frame,
                        target_width,
                        target_height,
                        &config.frame_output,
                    )?;
                    handler(current_frame_number, image, info)?;
                    target_index += 1;
                }
            }
        }

        if target_index < sorted_numbers.len() {
            decoder.send_eof()?;
            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                if target_index >= sorted_numbers.len() {
                    break;
                }
                if config.is_cancelled() {
                    return Err(UnbundleError::Cancelled);
                }

                let pts = decoded_frame.pts().unwrap_or(0);
                let current_frame_number =
                    crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

                while target_index < sorted_numbers.len()
                    && sorted_numbers[target_index] < current_frame_number
                {
                    target_index += 1;
                }

                if target_index < sorted_numbers.len()
                    && current_frame_number == sorted_numbers[target_index]
                {
                    let info = build_frame_info(&decoded_frame, current_frame_number, time_base);
                    let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    ensure_scaler(
                        &mut scaler,
                        source,
                        output_pixel,
                        target_width,
                        target_height,
                    )?;
                    scaler.as_mut().unwrap().run(source, &mut scaled_frame)?;
                    let image = convert_frame_to_image(
                        &scaled_frame,
                        target_width,
                        target_height,
                        &config.frame_output,
                    )?;
                    handler(current_frame_number, image, info)?;
                    target_index += 1;
                }
            }
        }

        Ok(())
    }

    /// Decode a contiguous range of frames and pass each to the handler.
    fn process_frame_range<F>(
        &mut self,
        start: u64,
        end: u64,
        video_metadata: &VideoMetadata,
        config: &ExtractOptions,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage) -> Result<(), UnbundleError>,
    {
        let video_stream_index = self.resolve_video_stream_index()?;
        log::debug!(
            "Processing frame range {}..={} (stream={})",
            start,
            end,
            video_stream_index
        );

        let (target_width, target_height) = config
            .frame_output
            .resolve_dimensions(video_metadata.width, video_metadata.height);
        let output_pixel = config.frame_output.pixel_format.to_ffmpeg_pixel();
        let frames_per_second = video_metadata.frames_per_second;

        let stream = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?;
        let time_base = stream.time_base();
        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let (mut decoder, hw_active) = create_video_decoder(decoder_context, config)?;

        // Defer scaler creation when HW accel is active — the software pixel
        // format is only known after the first frame transfer.
        let mut scaler: Option<ScalingContext> = if hw_active {
            None
        } else {
            Some(ScalingContext::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                output_pixel,
                target_width,
                target_height,
                ScalingFlags::BILINEAR,
            )?)
        };

        // Seek to start frame.
        let seek_timestamp =
            crate::conversion::frame_number_to_seek_timestamp(start, frames_per_second);
        self.unbundler
            .input_context
            .seek(seek_timestamp, ..seek_timestamp)?;

        let mut decoded_frame = VideoFrame::empty();
        let mut scaled_frame = VideoFrame::empty();

        for (stream, packet) in self.unbundler.input_context.packets() {
            if config.is_cancelled() {
                return Err(UnbundleError::Cancelled);
            }

            if stream.index() != video_stream_index {
                continue;
            }

            decoder.send_packet(&packet)?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                let pts = decoded_frame.pts().unwrap_or(0);
                let current_frame_number =
                    crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

                if current_frame_number >= start && current_frame_number <= end {
                    let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    ensure_scaler(
                        &mut scaler,
                        source,
                        output_pixel,
                        target_width,
                        target_height,
                    )?;
                    scaler.as_mut().unwrap().run(source, &mut scaled_frame)?;
                    let image = convert_frame_to_image(
                        &scaled_frame,
                        target_width,
                        target_height,
                        &config.frame_output,
                    )?;
                    handler(current_frame_number, image)?;
                }

                if current_frame_number > end {
                    return Ok(());
                }
            }
        }

        // Flush the decoder.
        decoder.send_eof()?;
        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            if config.is_cancelled() {
                return Err(UnbundleError::Cancelled);
            }

            let pts = decoded_frame.pts().unwrap_or(0);
            let current_frame_number =
                crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

            if current_frame_number >= start && current_frame_number <= end {
                let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                let source = transferred.as_ref().unwrap_or(&decoded_frame);
                ensure_scaler(
                    &mut scaler,
                    source,
                    output_pixel,
                    target_width,
                    target_height,
                )?;
                scaler.as_mut().unwrap().run(source, &mut scaled_frame)?;
                let image = convert_frame_to_image(
                    &scaled_frame,
                    target_width,
                    target_height,
                    &config.frame_output,
                )?;
                handler(current_frame_number, image)?;
            }

            if current_frame_number > end {
                break;
            }
        }

        Ok(())
    }

    /// Process frames at specific (possibly non-contiguous) frame numbers.
    ///
    /// Sorts the requested frame numbers and processes them in order to
    /// minimise seeks. Sequential runs are decoded without re-seeking.
    fn process_specific_frames<F>(
        &mut self,
        frame_numbers: &[u64],
        video_metadata: &VideoMetadata,
        config: &ExtractOptions,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage) -> Result<(), UnbundleError>,
    {
        if frame_numbers.is_empty() {
            return Ok(());
        }

        let video_stream_index = self.resolve_video_stream_index()?;
        log::debug!(
            "Processing {} specific frames (stream={})",
            frame_numbers.len(),
            video_stream_index
        );

        let (target_width, target_height) = config
            .frame_output
            .resolve_dimensions(video_metadata.width, video_metadata.height);
        let output_pixel = config.frame_output.pixel_format.to_ffmpeg_pixel();
        let frames_per_second = video_metadata.frames_per_second;

        // Sort frame numbers for sequential access.
        let mut sorted_numbers = frame_numbers.to_vec();
        sorted_numbers.sort_unstable();
        sorted_numbers.dedup();

        let stream = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?;
        let time_base = stream.time_base();
        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let (mut decoder, hw_active) = create_video_decoder(decoder_context, config)?;

        let mut scaler: Option<ScalingContext> = if hw_active {
            None
        } else {
            Some(ScalingContext::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                output_pixel,
                target_width,
                target_height,
                ScalingFlags::BILINEAR,
            )?)
        };

        // Seek to the first requested frame.
        let seek_timestamp =
            crate::conversion::frame_number_to_seek_timestamp(sorted_numbers[0], frames_per_second);
        self.unbundler
            .input_context
            .seek(seek_timestamp, ..seek_timestamp)?;

        let mut target_index = 0;
        let mut decoded_frame = VideoFrame::empty();
        let mut scaled_frame = VideoFrame::empty();

        for (stream, packet) in self.unbundler.input_context.packets() {
            if target_index >= sorted_numbers.len() {
                break;
            }
            if config.is_cancelled() {
                return Err(UnbundleError::Cancelled);
            }
            if stream.index() != video_stream_index {
                continue;
            }

            decoder.send_packet(&packet)?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                if target_index >= sorted_numbers.len() {
                    break;
                }

                let pts = decoded_frame.pts().unwrap_or(0);
                let current_frame_number =
                    crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

                // Skip target numbers that are before the current position
                // (can happen after a seek lands past the target).
                while target_index < sorted_numbers.len()
                    && sorted_numbers[target_index] < current_frame_number
                {
                    target_index += 1;
                }

                if target_index < sorted_numbers.len()
                    && current_frame_number == sorted_numbers[target_index]
                {
                    let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    ensure_scaler(
                        &mut scaler,
                        source,
                        output_pixel,
                        target_width,
                        target_height,
                    )?;
                    scaler.as_mut().unwrap().run(source, &mut scaled_frame)?;
                    let image = convert_frame_to_image(
                        &scaled_frame,
                        target_width,
                        target_height,
                        &config.frame_output,
                    )?;
                    handler(current_frame_number, image)?;
                    target_index += 1;
                }
            }
        }

        // Flush the decoder for any remaining frames.
        if target_index < sorted_numbers.len() {
            decoder.send_eof()?;
            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                if target_index >= sorted_numbers.len() {
                    break;
                }

                if config.is_cancelled() {
                    return Err(UnbundleError::Cancelled);
                }

                let pts = decoded_frame.pts().unwrap_or(0);
                let current_frame_number =
                    crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

                while target_index < sorted_numbers.len()
                    && sorted_numbers[target_index] < current_frame_number
                {
                    target_index += 1;
                }

                if target_index < sorted_numbers.len()
                    && current_frame_number == sorted_numbers[target_index]
                {
                    let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    ensure_scaler(
                        &mut scaler,
                        source,
                        output_pixel,
                        target_width,
                        target_height,
                    )?;
                    scaler.as_mut().unwrap().run(source, &mut scaled_frame)?;
                    let image = convert_frame_to_image(
                        &scaled_frame,
                        target_width,
                        target_height,
                        &config.frame_output,
                    )?;
                    handler(current_frame_number, image)?;
                    target_index += 1;
                }
            }
        }

        Ok(())
    }
}

/// Create a video decoder, optionally with hardware acceleration.
///
/// Returns `(decoder, hw_active)` where `hw_active` indicates whether
/// hardware decoding was successfully initialised.
fn create_video_decoder(
    codec_context: CodecContext,
    #[allow(unused_variables)] config: &ExtractOptions,
) -> Result<(VideoDecoder, bool), UnbundleError> {
    #[cfg(feature = "hardware")]
    {
        let setup = crate::hardware_acceleration::try_create_hw_decoder(
            codec_context,
            config.hardware_acceleration,
        )?;
        Ok((setup.decoder, setup.hw_active))
    }
    #[cfg(not(feature = "hardware"))]
    {
        let decoder = codec_context.decoder().video()?;
        Ok((decoder, false))
    }
}

/// If HW decoding is active, transfer a decoded frame from GPU to system
/// memory.  Returns `Some(sw_frame)` on successful transfer, `None` when
/// the frame is already in system memory or when HW accel is not enabled.
fn maybe_transfer_hw_frame(
    #[allow(unused_variables)] frame: &VideoFrame,
    #[allow(unused_variables)] hw_active: bool,
) -> Result<Option<VideoFrame>, UnbundleError> {
    #[cfg(feature = "hardware")]
    if hw_active {
        match crate::hardware_acceleration::transfer_hw_frame(frame) {
            Ok(sw_frame) => return Ok(Some(sw_frame)),
            Err(_) => return Ok(None), // frame already in system memory
        }
    }
    Ok(None)
}

/// Lazily initialise the software scaler on the first decoded frame.
///
/// When hardware decoding is in use the decoder reports a hardware pixel
/// format that the software scaler cannot process.  This function creates
/// the scaler from the actual (transferred) frame dimensions and format.
fn ensure_scaler(
    scaler: &mut Option<ScalingContext>,
    source: &VideoFrame,
    output_pixel: Pixel,
    target_width: u32,
    target_height: u32,
) -> Result<(), UnbundleError> {
    if scaler.is_none() {
        *scaler = Some(ScalingContext::get(
            source.format(),
            source.width(),
            source.height(),
            output_pixel,
            target_width,
            target_height,
            ScalingFlags::BILINEAR,
        )?);
    }
    Ok(())
}

/// Convert a scaled video frame to an [`image::DynamicImage`].
///
/// Supports RGB24, RGBA, and GRAY8 output depending on the
/// [`FrameOutputOptions`].
fn convert_frame_to_image(
    frame: &VideoFrame,
    width: u32,
    height: u32,
    output_config: &FrameOutputOptions,
) -> Result<DynamicImage, UnbundleError> {
    match output_config.pixel_format {
        PixelFormat::Rgb8 => {
            let buffer = crate::conversion::frame_to_buffer(frame, width, height, 3);
            let rgb_image = RgbImage::from_raw(width, height, buffer).ok_or_else(|| {
                UnbundleError::VideoDecodeError(
                    "Failed to construct RGB image from decoded frame data".to_string(),
                )
            })?;
            Ok(DynamicImage::ImageRgb8(rgb_image))
        }
        PixelFormat::Rgba8 => {
            let buffer = crate::conversion::frame_to_buffer(frame, width, height, 4);
            let rgba_image = RgbaImage::from_raw(width, height, buffer).ok_or_else(|| {
                UnbundleError::VideoDecodeError(
                    "Failed to construct RGBA image from decoded frame data".to_string(),
                )
            })?;
            Ok(DynamicImage::ImageRgba8(rgba_image))
        }
        PixelFormat::Gray8 => {
            let buffer = crate::conversion::frame_to_buffer(frame, width, height, 1);
            let gray_image = GrayImage::from_raw(width, height, buffer).ok_or_else(|| {
                UnbundleError::VideoDecodeError(
                    "Failed to construct grayscale image from decoded frame data".to_string(),
                )
            })?;
            Ok(DynamicImage::ImageLuma8(gray_image))
        }
    }
}

/// Build a [`FrameMetadata`] from a decoded video frame.
fn build_frame_info(frame: &VideoFrame, frame_number: u64, time_base: Rational) -> FrameMetadata {
    let pts = frame.pts();
    let timestamp_seconds = crate::conversion::pts_to_seconds(pts.unwrap_or(0), time_base);
    let timestamp = Duration::from_secs_f64(timestamp_seconds.max(0.0));

    FrameMetadata {
        frame_number,
        timestamp,
        pts,
        is_keyframe: frame.is_key(),
        frame_type: picture_type_to_frame_type(frame.kind()),
    }
}

/// Convert FFmpeg's [`PictureType`] to our public [`FrameType`] enum.
fn picture_type_to_frame_type(ptype: PictureType) -> FrameType {
    match ptype {
        PictureType::I => FrameType::I,
        PictureType::P => FrameType::P,
        PictureType::B => FrameType::B,
        PictureType::S => FrameType::S,
        PictureType::SI => FrameType::SI,
        PictureType::SP => FrameType::SP,
        PictureType::BI => FrameType::BI,
        _ => FrameType::Unknown,
    }
}
