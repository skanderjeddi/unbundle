//! Video frame extraction.
//!
//! This module provides [`VideoExtractor`] for extracting still frames from
//! video files, and [`FrameRange`] for specifying which frames to extract.
//! Extracted frames are returned as [`image::DynamicImage`] values that can be
//! saved, manipulated, or converted to other formats.

use std::path::Path;
use std::time::Duration;

use ffmpeg_next::{
    codec::context::Context as CodecContext,
    decoder::Video as VideoDecoder,
    format::Pixel,
    frame::Video as VideoFrame,
    software::scaling::{Context as ScalingContext, Flags as ScalingFlags},
    util::picture::Type as PictureType,
};
use image::{DynamicImage, GrayImage, RgbImage, RgbaImage};

use crate::{
    config::{ExtractionConfig, FrameOutputConfig, PixelFormat},
    error::UnbundleError,
    iterator::FrameIterator,
    metadata::VideoMetadata,
    progress::{OperationType, ProgressTracker},
    unbundler::MediaUnbundler,
};
#[cfg(feature = "async-tokio")]
use crate::stream::FrameStream;

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
/// [`VideoExtractor::frame_with_info`] and
/// [`VideoExtractor::frames_with_info`].
///
/// # Example
///
/// ```no_run
/// use unbundle::MediaUnbundler;
///
/// let mut unbundler = MediaUnbundler::open("input.mp4")?;
/// let (image, info) = unbundler.video().frame_with_info(0)?;
/// println!("Frame {} at {:?}, keyframe={}", info.frame_number,
///     info.timestamp, info.is_keyframe);
/// # Ok::<(), unbundle::UnbundleError>(())
/// ```
#[derive(Debug, Clone)]
pub struct FrameInfo {
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
/// Used with [`VideoExtractor::frames`] to extract multiple frames in a single
/// call.
///
/// # Example
///
/// ```no_run
/// use std::time::Duration;
///
/// use unbundle::{FrameRange, MediaUnbundler};
///
/// let mut unbundler = MediaUnbundler::open("input.mp4").unwrap();
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
    /// use unbundle::{FrameRange, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let frames = unbundler.video().frames(FrameRange::Segments(vec![
    ///     (Duration::from_secs(0), Duration::from_secs(2)),
    ///     (Duration::from_secs(10), Duration::from_secs(12)),
    /// ]))?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    Segments(Vec<(Duration, Duration)>),
}

/// Video frame extraction operations.
///
/// Obtained via [`MediaUnbundler::video`]. Each extraction method creates a
/// fresh decoder, seeks to the relevant position, and decodes frames. The
/// decoder is dropped when the method returns.
///
/// Frames are returned as [`DynamicImage`] in RGB8 format.
pub struct VideoExtractor<'a> {
    pub(crate) unbundler: &'a mut MediaUnbundler,
}

impl<'a> VideoExtractor<'a> {
    /// Extract a single frame by frame number (0-indexed).
    ///
    /// Seeks to the nearest keyframe before the target and decodes forward
    /// until the requested frame is reached.
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
    /// use unbundle::MediaUnbundler;
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let frame = unbundler.video().frame(100)?;
    /// frame.save("frame_100.png")?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn frame(&mut self, frame_number: u64) -> Result<DynamicImage, UnbundleError> {
        let video_stream_index = self
            .unbundler
            .video_stream_index
            .ok_or(UnbundleError::NoVideoStream)?;

        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?;

        let total_frames = video_metadata.frame_count;
        let frames_per_second = video_metadata.frames_per_second;
        let target_width = video_metadata.width;
        let target_height = video_metadata.height;

        if total_frames > 0 && frame_number >= total_frames {
            return Err(UnbundleError::FrameOutOfRange {
                frame_number,
                total_frames,
            });
        }

        // Build a fresh decoder from the stream parameters.
        let stream = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?;
        let time_base = stream.time_base();
        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let mut decoder = decoder_context.decoder().video()?;

        // Set up the pixel-format converter (source format → RGB24).
        let mut scaler = ScalingContext::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::RGB24,
            target_width,
            target_height,
            ScalingFlags::BILINEAR,
        )?;

        // Seek to the nearest keyframe before the target frame.
        let target_timestamp = crate::utilities::frame_number_to_stream_timestamp(
            frame_number,
            frames_per_second,
            time_base,
        );

        // Seek in the stream's time base.
        self.unbundler
            .input_context
            .seek(target_timestamp, ..target_timestamp)?;

        // Decode frames until we reach the target.
        let mut decoded_frame = VideoFrame::empty();
        let mut rgb_frame = VideoFrame::empty();
        let default_output = FrameOutputConfig::default();

        for (stream, packet) in self.unbundler.input_context.packets() {
            if stream.index() != video_stream_index {
                continue;
            }

            decoder.send_packet(&packet)?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                let pts = decoded_frame.pts().unwrap_or(0);
                let current_frame_number =
                    crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

                if current_frame_number == frame_number {
                    scaler.run(&decoded_frame, &mut rgb_frame)?;
                    return convert_frame_to_image(
                        &rgb_frame,
                        target_width,
                        target_height,
                        &default_output,
                    );
                }

                // If we've gone past the target, the frame doesn't exist at
                // this exact index — return the closest frame after a seek.
                if current_frame_number > frame_number {
                    scaler.run(&decoded_frame, &mut rgb_frame)?;
                    return convert_frame_to_image(
                        &rgb_frame,
                        target_width,
                        target_height,
                        &default_output,
                    );
                }
            }
        }

        // Flush the decoder.
        decoder.send_eof()?;
        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            let pts = decoded_frame.pts().unwrap_or(0);
            let current_frame_number =
                crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

            if current_frame_number >= frame_number {
                scaler.run(&decoded_frame, &mut rgb_frame)?;
                return convert_frame_to_image(
                    &rgb_frame,
                    target_width,
                    target_height,
                    &default_output,
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
    /// and delegates to [`frame`](VideoExtractor::frame).
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::InvalidTimestamp`] if the timestamp exceeds the
    /// media duration, or any error from [`frame`](VideoExtractor::frame).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::MediaUnbundler;
    /// use std::time::Duration;
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let frame = unbundler.video().frame_at(Duration::from_secs(30))?;
    /// frame.save("frame_at_30s.png")?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn frame_at(&mut self, timestamp: Duration) -> Result<DynamicImage, UnbundleError> {
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
            crate::utilities::timestamp_to_frame_number(timestamp, frames_per_second);
        self.frame(frame_number)
    }

    /// Extract a single frame by number, returning both the image and its
    /// [`FrameInfo`] metadata.
    ///
    /// This combines frame extraction with metadata collection (PTS,
    /// keyframe flag, picture type) in a single decode pass.
    ///
    /// # Errors
    ///
    /// Same as [`frame`](VideoExtractor::frame).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::MediaUnbundler;
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let (image, info) = unbundler.video().frame_with_info(42)?;
    /// println!("PTS: {:?}, keyframe: {}", info.pts, info.is_keyframe);
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn frame_with_info(
        &mut self,
        frame_number: u64,
    ) -> Result<(DynamicImage, FrameInfo), UnbundleError> {
        let video_stream_index = self
            .unbundler
            .video_stream_index
            .ok_or(UnbundleError::NoVideoStream)?;

        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?;

        let total_frames = video_metadata.frame_count;
        let frames_per_second = video_metadata.frames_per_second;
        let target_width = video_metadata.width;
        let target_height = video_metadata.height;

        if total_frames > 0 && frame_number >= total_frames {
            return Err(UnbundleError::FrameOutOfRange {
                frame_number,
                total_frames,
            });
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
            Pixel::RGB24,
            target_width,
            target_height,
            ScalingFlags::BILINEAR,
        )?;

        let target_timestamp = crate::utilities::frame_number_to_stream_timestamp(
            frame_number,
            frames_per_second,
            time_base,
        );

        self.unbundler
            .input_context
            .seek(target_timestamp, ..target_timestamp)?;

        let mut decoded_frame = VideoFrame::empty();
        let mut rgb_frame = VideoFrame::empty();
        let default_output = FrameOutputConfig::default();

        for (stream, packet) in self.unbundler.input_context.packets() {
            if stream.index() != video_stream_index {
                continue;
            }

            decoder.send_packet(&packet)?;

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                let pts = decoded_frame.pts().unwrap_or(0);
                let current_frame_number =
                    crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

                if current_frame_number >= frame_number {
                    let info = build_frame_info(
                        &decoded_frame,
                        current_frame_number,
                        time_base,
                    );
                    scaler.run(&decoded_frame, &mut rgb_frame)?;
                    let image = convert_frame_to_image(
                        &rgb_frame,
                        target_width,
                        target_height,
                        &default_output,
                    )?;
                    return Ok((image, info));
                }
            }
        }

        decoder.send_eof()?;
        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            let pts = decoded_frame.pts().unwrap_or(0);
            let current_frame_number =
                crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

            if current_frame_number >= frame_number {
                let info = build_frame_info(
                    &decoded_frame,
                    current_frame_number,
                    time_base,
                );
                scaler.run(&decoded_frame, &mut rgb_frame)?;
                let image = convert_frame_to_image(
                    &rgb_frame,
                    target_width,
                    target_height,
                    &default_output,
                )?;
                return Ok((image, info));
            }
        }

        Err(UnbundleError::VideoDecodeError(format!(
            "Could not locate frame {frame_number} in the video stream"
        )))
    }

    /// Extract multiple frames with their [`FrameInfo`] metadata.
    ///
    /// Like [`frames`](VideoExtractor::frames) but returns
    /// `(DynamicImage, FrameInfo)` pairs, giving access to PTS, keyframe
    /// flags, and picture types for every extracted frame.
    ///
    /// # Errors
    ///
    /// Same as [`frames`](VideoExtractor::frames).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{FrameRange, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let results = unbundler.video().frames_with_info(FrameRange::Range(0, 9))?;
    /// for (image, info) in &results {
    ///     println!("Frame {} — type {:?}", info.frame_number, info.frame_type);
    /// }
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn frames_with_info(
        &mut self,
        range: FrameRange,
    ) -> Result<Vec<(DynamicImage, FrameInfo)>, UnbundleError> {
        self.frames_with_info_config(range, &ExtractionConfig::default())
    }

    /// Extract multiple frames with [`FrameInfo`] and progress/cancellation.
    ///
    /// Like [`frames_with_config`](VideoExtractor::frames_with_config) but
    /// includes [`FrameInfo`] for each frame.
    ///
    /// # Errors
    ///
    /// Same as [`frames_with_config`](VideoExtractor::frames_with_config).
    pub fn frames_with_info_config(
        &mut self,
        range: FrameRange,
        config: &ExtractionConfig,
    ) -> Result<Vec<(DynamicImage, FrameInfo)>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let total = Self::estimate_frame_count(
            &range,
            &video_metadata,
            self.unbundler.metadata.duration,
        );

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
    /// Convenience method that combines [`frame`](VideoExtractor::frame) with
    /// [`DynamicImage::save`]. The output format is inferred from the file
    /// extension.
    ///
    /// # Errors
    ///
    /// Returns errors from [`frame`](VideoExtractor::frame), or
    /// [`UnbundleError::ImageError`] if the image cannot be written.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::MediaUnbundler;
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// unbundler.video().save_frame(0, "first_frame.png")?;
    /// # Ok::<(), unbundle::UnbundleError>(())
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
    /// Convenience method that combines [`frame_at`](VideoExtractor::frame_at)
    /// with [`DynamicImage::save`]. The output format is inferred from the file
    /// extension.
    ///
    /// # Errors
    ///
    /// Returns errors from [`frame_at`](VideoExtractor::frame_at), or
    /// [`UnbundleError::ImageError`] if the image cannot be written.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use unbundle::MediaUnbundler;
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// unbundler.video().save_frame_at(Duration::from_secs(5), "frame_5s.png")?;
    /// # Ok::<(), unbundle::UnbundleError>(())
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
    /// use unbundle::{FrameRange, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let frames = unbundler.video().frames(FrameRange::Range(0, 9))?;
    /// assert_eq!(frames.len(), 10);
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn frames(&mut self, range: FrameRange) -> Result<Vec<DynamicImage>, UnbundleError> {
        self.frames_with_config(range, &ExtractionConfig::default())
    }

    /// Process frames one at a time without collecting them into a `Vec`.
    ///
    /// This is a streaming alternative to [`frames`](VideoExtractor::frames)
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
    /// use unbundle::{FrameRange, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// unbundler.video().for_each_frame(
    ///     FrameRange::Range(0, 9),
    ///     |frame_number, image| {
    ///         image.save(format!("frame_{frame_number}.png"))?;
    ///         Ok(())
    ///     },
    /// )?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn for_each_frame<F>(
        &mut self,
        range: FrameRange,
        callback: F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage) -> Result<(), UnbundleError>,
    {
        self.for_each_frame_with_config(range, &ExtractionConfig::default(), callback)
    }

    /// Extract multiple frames with progress reporting and cancellation.
    ///
    /// Like [`frames`](VideoExtractor::frames) but accepts an
    /// [`ExtractionConfig`] for progress callbacks and cancellation support.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested,
    /// or any error from [`frames`](VideoExtractor::frames).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::sync::Arc;
    ///
    /// use unbundle::{ExtractionConfig, FrameRange, MediaUnbundler, ProgressCallback, ProgressInfo};
    ///
    /// struct PrintProgress;
    /// impl ProgressCallback for PrintProgress {
    ///     fn on_progress(&self, info: &ProgressInfo) {
    ///         println!("Frame {}/{}", info.current, info.total.unwrap_or(0));
    ///     }
    /// }
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let config = ExtractionConfig::new()
    ///     .with_progress(Arc::new(PrintProgress));
    /// let frames = unbundler.video().frames_with_config(
    ///     FrameRange::Range(0, 9),
    ///     &config,
    /// )?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn frames_with_config(
        &mut self,
        range: FrameRange,
        config: &ExtractionConfig,
    ) -> Result<Vec<DynamicImage>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let total = Self::estimate_frame_count(&range, &video_metadata, self.unbundler.metadata.duration);

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
    /// Like [`for_each_frame`](VideoExtractor::for_each_frame) but accepts an
    /// [`ExtractionConfig`].
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested,
    /// or any error from decoding or the callback.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{CancellationToken, ExtractionConfig, FrameRange, MediaUnbundler};
    ///
    /// let token = CancellationToken::new();
    /// let config = ExtractionConfig::new()
    ///     .with_cancellation(token.clone());
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// unbundler.video().for_each_frame_with_config(
    ///     FrameRange::Range(0, 99),
    ///     &config,
    ///     |frame_number, image| {
    ///         image.save(format!("frame_{frame_number}.png"))?;
    ///         Ok(())
    ///     },
    /// )?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn for_each_frame_with_config<F>(
        &mut self,
        range: FrameRange,
        config: &ExtractionConfig,
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

        let total = Self::estimate_frame_count(&range, &video_metadata, self.unbundler.metadata.duration);

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
    /// An optional [`SceneDetectionConfig`](crate::scene::SceneDetectionConfig)
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
    /// use unbundle::MediaUnbundler;
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let scenes = unbundler.video().detect_scenes(None)?;
    /// println!("Found {} scene changes", scenes.len());
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    #[cfg(feature = "scene-detection")]
    pub fn detect_scenes(
        &mut self,
        config: Option<crate::scene::SceneDetectionConfig>,
    ) -> Result<Vec<crate::scene::SceneChange>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let scd_config = config.unwrap_or_default();
        crate::scene::detect_scenes_impl(self.unbundler, &video_metadata, &scd_config, None)
    }

    /// Detect scene changes with cancellation support.
    ///
    /// Like [`detect_scenes`](VideoExtractor::detect_scenes) but accepts an
    /// [`ExtractionConfig`] for cancellation.
    #[cfg(feature = "scene-detection")]
    pub fn detect_scenes_with_config(
        &mut self,
        scd_config: Option<crate::scene::SceneDetectionConfig>,
        config: &ExtractionConfig,
    ) -> Result<Vec<crate::scene::SceneChange>, UnbundleError> {
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
    /// use unbundle::{ExtractionConfig, FrameRange, MediaUnbundler};
    ///
    /// # async fn example() -> Result<(), unbundle::UnbundleError> {
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let mut stream = unbundler
    ///     .video()
    ///     .frame_stream(FrameRange::Range(0, 9), ExtractionConfig::new())?;
    ///
    /// while let Some(result) = stream.next().await {
    ///     let (frame_number, image) = result?;
    ///     image.save(format!("frame_{frame_number}.png"))?;
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "async-tokio")]
    pub fn frame_stream(
        &mut self,
        range: FrameRange,
        config: ExtractionConfig,
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
    /// Unlike [`frames`](VideoExtractor::frames), which decodes everything
    /// up-front, this returns a [`FrameIterator`]
    /// that decodes one frame at a time on each [`next()`](Iterator::next)
    /// call. This is ideal when you want to stop early or process frames
    /// one by one without buffering the entire set.
    ///
    /// The iterator borrows the underlying [`MediaUnbundler`] mutably and
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
    /// use unbundle::{FrameRange, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let iter = unbundler.video().frame_iter(FrameRange::Range(0, 9))?;
    ///
    /// for result in iter {
    ///     let (frame_number, image) = result?;
    ///     image.save(format!("frame_{frame_number}.png"))?;
    /// }
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn frame_iter(
        self,
        range: FrameRange,
    ) -> Result<FrameIterator<'a>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let frame_numbers = self.resolve_frame_numbers_for_iter(range, &video_metadata)?;
        let output_config = FrameOutputConfig::default();

        FrameIterator::new(self.unbundler, frame_numbers, output_config)
    }

    /// Create a lazy iterator with custom output configuration.
    ///
    /// Like [`frame_iter`](VideoExtractor::frame_iter) but uses the given
    /// [`FrameOutputConfig`] for pixel format and resolution settings.
    ///
    /// # Errors
    ///
    /// Returns errors from [`frame_iter`](VideoExtractor::frame_iter).
    pub fn frame_iter_with_config(
        self,
        range: FrameRange,
        output_config: FrameOutputConfig,
    ) -> Result<FrameIterator<'a>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let frame_numbers = self.resolve_frame_numbers_for_iter(range, &video_metadata)?;
        FrameIterator::new(self.unbundler, frame_numbers, output_config)
    }

    /// Resolve a [`FrameRange`] into sorted, deduplicated frame numbers.
    ///
    /// Shared helper for [`frame_iter`](VideoExtractor::frame_iter) and
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
                let start_frame = crate::utilities::timestamp_to_frame_number(
                    start_time,
                    video_metadata.frames_per_second,
                );
                let end_frame = crate::utilities::timestamp_to_frame_number(
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
                    nums.push(crate::utilities::timestamp_to_frame_number(
                        current,
                        video_metadata.frames_per_second,
                    ));
                    current += interval;
                }
                nums
            }
            FrameRange::Specific(nums) => nums,
            FrameRange::Segments(segments) => {
                Self::resolve_segments(&segments, video_metadata)?
            }
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
            let start_frame = crate::utilities::timestamp_to_frame_number(
                *start,
                video_metadata.frames_per_second,
            );
            let end_frame = crate::utilities::timestamp_to_frame_number(
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
    /// use unbundle::{ExtractionConfig, FrameRange, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let config = ExtractionConfig::new();
    /// let frames = unbundler
    ///     .video()
    ///     .frames_parallel(FrameRange::Interval(100), &config)?;
    /// println!("Got {} frames", frames.len());
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    #[cfg(feature = "parallel")]
    pub fn frames_parallel(
        &mut self,
        range: FrameRange,
        config: &ExtractionConfig,
    ) -> Result<Vec<DynamicImage>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        // Resolve the range into concrete frame numbers.
        let frame_numbers = self.resolve_frame_numbers(range, &video_metadata)?;

        let results = crate::parallel::parallel_extract_frames(
            &self.unbundler.file_path,
            &frame_numbers,
            &video_metadata,
            config,
        )?;

        Ok(results.into_iter().map(|(_, img)| img).collect())
    }

    /// Resolve a [`FrameRange`] into a sorted, deduplicated list of frame
    /// numbers.
    #[cfg(feature = "parallel")]
    fn resolve_frame_numbers(
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
                let start_frame = crate::utilities::timestamp_to_frame_number(
                    start_time,
                    video_metadata.frames_per_second,
                );
                let end_frame = crate::utilities::timestamp_to_frame_number(
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
                    nums.push(crate::utilities::timestamp_to_frame_number(
                        current,
                        video_metadata.frames_per_second,
                    ));
                    current += interval;
                }
                nums
            }
            FrameRange::Specific(nums) => nums,
            FrameRange::Segments(segments) => {
                Self::resolve_segments(&segments, video_metadata)?
            }
        };
        numbers.sort_unstable();
        numbers.dedup();
        Ok(numbers)
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
                let start_frame =
                    crate::utilities::timestamp_to_frame_number(*start_time, fps);
                let end_frame =
                    crate::utilities::timestamp_to_frame_number(*end_time, fps);
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
                let total: u64 = segments.iter().map(|(start, end)| {
                    let sf = crate::utilities::timestamp_to_frame_number(*start, fps);
                    let ef = crate::utilities::timestamp_to_frame_number(*end, fps);
                    ef.saturating_sub(sf) + 1
                }).sum();
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
        config: &ExtractionConfig,
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
                let start_frame = crate::utilities::timestamp_to_frame_number(
                    start_time,
                    video_metadata.frames_per_second,
                );
                let end_frame = crate::utilities::timestamp_to_frame_number(
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
                    numbers.push(crate::utilities::timestamp_to_frame_number(
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

    /// Validate and dispatch a [`FrameRange`], passing [`FrameInfo`]
    /// alongside each decoded image.
    fn dispatch_range_with_info<F>(
        &mut self,
        range: FrameRange,
        video_metadata: &VideoMetadata,
        config: &ExtractionConfig,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage, FrameInfo) -> Result<(), UnbundleError>,
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
                self.process_specific_frames_with_info(
                    &numbers,
                    video_metadata,
                    config,
                    handler,
                )
            }
            FrameRange::TimeRange(start_time, end_time) => {
                if start_time >= end_time {
                    return Err(UnbundleError::InvalidRange {
                        start: format!("{start_time:?}"),
                        end: format!("{end_time:?}"),
                    });
                }
                let start_frame = crate::utilities::timestamp_to_frame_number(
                    start_time,
                    video_metadata.frames_per_second,
                );
                let end_frame = crate::utilities::timestamp_to_frame_number(
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
                    numbers.push(crate::utilities::timestamp_to_frame_number(
                        current,
                        video_metadata.frames_per_second,
                    ));
                    current += interval;
                }
                self.process_specific_frames_with_info(
                    &numbers,
                    video_metadata,
                    config,
                    handler,
                )
            }
            FrameRange::Specific(numbers) => {
                self.process_specific_frames_with_info(
                    &numbers,
                    video_metadata,
                    config,
                    handler,
                )
            }
            FrameRange::Segments(segments) => {
                let numbers = Self::resolve_segments(&segments, video_metadata)?;
                self.process_specific_frames_with_info(
                    &numbers,
                    video_metadata,
                    config,
                    handler,
                )
            }
        }
    }

    /// Decode a contiguous range of frames, calling the handler with
    /// [`FrameInfo`] for each.
    fn process_frame_range_with_info<F>(
        &mut self,
        start: u64,
        end: u64,
        video_metadata: &VideoMetadata,
        config: &ExtractionConfig,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage, FrameInfo) -> Result<(), UnbundleError>,
    {
        let video_stream_index = self
            .unbundler
            .video_stream_index
            .ok_or(UnbundleError::NoVideoStream)?;

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

        let start_timestamp =
            crate::utilities::frame_number_to_stream_timestamp(start, frames_per_second, time_base);
        self.unbundler
            .input_context
            .seek(start_timestamp, ..start_timestamp)?;

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
                    crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

                if current_frame_number >= start && current_frame_number <= end {
                    let info = build_frame_info(&decoded_frame, current_frame_number, time_base);
                    let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    ensure_scaler(
                        &mut scaler, source, output_pixel, target_width, target_height,
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
                crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

            if current_frame_number >= start && current_frame_number <= end {
                let info = build_frame_info(&decoded_frame, current_frame_number, time_base);
                let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                let source = transferred.as_ref().unwrap_or(&decoded_frame);
                ensure_scaler(
                    &mut scaler, source, output_pixel, target_width, target_height,
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
    /// passing [`FrameInfo`] alongside each decoded image.
    fn process_specific_frames_with_info<F>(
        &mut self,
        frame_numbers: &[u64],
        video_metadata: &VideoMetadata,
        config: &ExtractionConfig,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage, FrameInfo) -> Result<(), UnbundleError>,
    {
        if frame_numbers.is_empty() {
            return Ok(());
        }

        let video_stream_index = self
            .unbundler
            .video_stream_index
            .ok_or(UnbundleError::NoVideoStream)?;

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

        let first_timestamp = crate::utilities::frame_number_to_stream_timestamp(
            sorted_numbers[0],
            frames_per_second,
            time_base,
        );
        self.unbundler
            .input_context
            .seek(first_timestamp, ..first_timestamp)?;

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
                    crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

                while target_index < sorted_numbers.len()
                    && sorted_numbers[target_index] < current_frame_number
                {
                    target_index += 1;
                }

                if target_index < sorted_numbers.len()
                    && current_frame_number == sorted_numbers[target_index]
                {
                    let info = build_frame_info(
                        &decoded_frame,
                        current_frame_number,
                        time_base,
                    );
                    let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    ensure_scaler(
                        &mut scaler, source, output_pixel, target_width, target_height,
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
                    crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

                while target_index < sorted_numbers.len()
                    && sorted_numbers[target_index] < current_frame_number
                {
                    target_index += 1;
                }

                if target_index < sorted_numbers.len()
                    && current_frame_number == sorted_numbers[target_index]
                {
                    let info = build_frame_info(
                        &decoded_frame,
                        current_frame_number,
                        time_base,
                    );
                    let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    ensure_scaler(
                        &mut scaler, source, output_pixel, target_width, target_height,
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
        config: &ExtractionConfig,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage) -> Result<(), UnbundleError>,
    {
        let video_stream_index = self
            .unbundler
            .video_stream_index
            .ok_or(UnbundleError::NoVideoStream)?;

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
        let start_timestamp =
            crate::utilities::frame_number_to_stream_timestamp(start, frames_per_second, time_base);
        self.unbundler
            .input_context
            .seek(start_timestamp, ..start_timestamp)?;

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
                    crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

                if current_frame_number >= start && current_frame_number <= end {
                    let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    ensure_scaler(
                        &mut scaler, source, output_pixel, target_width, target_height,
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
                crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

            if current_frame_number >= start && current_frame_number <= end {
                let transferred = maybe_transfer_hw_frame(&decoded_frame, hw_active)?;
                let source = transferred.as_ref().unwrap_or(&decoded_frame);
                ensure_scaler(
                    &mut scaler, source, output_pixel, target_width, target_height,
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
        config: &ExtractionConfig,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, DynamicImage) -> Result<(), UnbundleError>,
    {
        if frame_numbers.is_empty() {
            return Ok(());
        }

        let video_stream_index = self
            .unbundler
            .video_stream_index
            .ok_or(UnbundleError::NoVideoStream)?;

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
        let first_timestamp = crate::utilities::frame_number_to_stream_timestamp(
            sorted_numbers[0],
            frames_per_second,
            time_base,
        );
        self.unbundler
            .input_context
            .seek(first_timestamp, ..first_timestamp)?;

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
                    crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

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
                        &mut scaler, source, output_pixel, target_width, target_height,
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
                    crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

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
                        &mut scaler, source, output_pixel, target_width, target_height,
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
    #[allow(unused_variables)] config: &ExtractionConfig,
) -> Result<(VideoDecoder, bool), UnbundleError> {
    #[cfg(feature = "hw-accel")]
    {
        let setup = crate::hw_accel::try_create_hw_decoder(codec_context, config.hw_accel)?;
        Ok((setup.decoder, setup.hw_active))
    }
    #[cfg(not(feature = "hw-accel"))]
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
    #[cfg(feature = "hw-accel")]
    if hw_active {
        match crate::hw_accel::transfer_hw_frame(frame) {
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
/// [`FrameOutputConfig`].
fn convert_frame_to_image(
    frame: &VideoFrame,
    width: u32,
    height: u32,
    output_config: &FrameOutputConfig,
) -> Result<DynamicImage, UnbundleError> {
    match output_config.pixel_format {
        PixelFormat::Rgb8 => {
            let buffer = crate::utilities::frame_to_buffer(frame, width, height, 3);
            let rgb_image = RgbImage::from_raw(width, height, buffer).ok_or_else(|| {
                UnbundleError::VideoDecodeError(
                    "Failed to construct RGB image from decoded frame data".to_string(),
                )
            })?;
            Ok(DynamicImage::ImageRgb8(rgb_image))
        }
        PixelFormat::Rgba8 => {
            let buffer = crate::utilities::frame_to_buffer(frame, width, height, 4);
            let rgba_image = RgbaImage::from_raw(width, height, buffer).ok_or_else(|| {
                UnbundleError::VideoDecodeError(
                    "Failed to construct RGBA image from decoded frame data".to_string(),
                )
            })?;
            Ok(DynamicImage::ImageRgba8(rgba_image))
        }
        PixelFormat::Gray8 => {
            let buffer = crate::utilities::frame_to_buffer(frame, width, height, 1);
            let gray_image = GrayImage::from_raw(width, height, buffer).ok_or_else(|| {
                UnbundleError::VideoDecodeError(
                    "Failed to construct grayscale image from decoded frame data".to_string(),
                )
            })?;
            Ok(DynamicImage::ImageLuma8(gray_image))
        }
    }
}

/// Build a [`FrameInfo`] from a decoded video frame.
fn build_frame_info(
    frame: &VideoFrame,
    frame_number: u64,
    time_base: ffmpeg_next::Rational,
) -> FrameInfo {
    let pts = frame.pts();
    let timestamp_seconds = crate::utilities::pts_to_seconds(
        pts.unwrap_or(0),
        time_base,
    );
    let timestamp = Duration::from_secs_f64(timestamp_seconds.max(0.0));

    FrameInfo {
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
