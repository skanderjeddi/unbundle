//! Video frame extraction.
//!
//! This module provides [`VideoExtractor`] for extracting still frames from
//! video files, and [`FrameRange`] for specifying which frames to extract.
//! Extracted frames are returned as [`image::DynamicImage`] values that can be
//! saved, manipulated, or converted to other formats.

use std::time::Duration;

use ffmpeg_next::{
    codec::context::Context as CodecContext,
    format::Pixel,
    frame::Video as VideoFrame,
    software::scaling::{Context as ScalingContext, Flags as ScalingFlags},
};
use image::{DynamicImage, RgbImage};

use crate::{error::UnbundleError, metadata::VideoMetadata, unbundler::MediaUnbundler};

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
                    return convert_frame_to_image(&rgb_frame, target_width, target_height);
                }

                // If we've gone past the target, the frame doesn't exist at
                // this exact index — return the closest frame after a seek.
                if current_frame_number > frame_number {
                    scaler.run(&decoded_frame, &mut rgb_frame)?;
                    return convert_frame_to_image(&rgb_frame, target_width, target_height);
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
                return convert_frame_to_image(&rgb_frame, target_width, target_height);
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
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        match range {
            FrameRange::Range(start, end) => self.extract_frame_range(start, end, &video_metadata),
            FrameRange::Interval(step) => {
                let total = video_metadata.frame_count;
                let numbers: Vec<u64> = (0..total).step_by(step as usize).collect();
                self.extract_specific_frames(&numbers, &video_metadata)
            }
            FrameRange::TimeRange(start_time, end_time) => {
                let start_frame = crate::utilities::timestamp_to_frame_number(
                    start_time,
                    video_metadata.frames_per_second,
                );
                let end_frame = crate::utilities::timestamp_to_frame_number(
                    end_time,
                    video_metadata.frames_per_second,
                );
                self.extract_frame_range(start_frame, end_frame, &video_metadata)
            }
            FrameRange::TimeInterval(interval) => {
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
                self.extract_specific_frames(&numbers, &video_metadata)
            }
            FrameRange::Specific(numbers) => {
                self.extract_specific_frames(&numbers, &video_metadata)
            }
        }
    }

    /// Extract a contiguous range of frames sequentially (no per-frame seeking).
    fn extract_frame_range(
        &mut self,
        start: u64,
        end: u64,
        video_metadata: &VideoMetadata,
    ) -> Result<Vec<DynamicImage>, UnbundleError> {
        let video_stream_index = self
            .unbundler
            .video_stream_index
            .ok_or(UnbundleError::NoVideoStream)?;

        let target_width = video_metadata.width;
        let target_height = video_metadata.height;
        let frames_per_second = video_metadata.frames_per_second;

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

        // Seek to start frame.
        let start_timestamp =
            crate::utilities::frame_number_to_stream_timestamp(start, frames_per_second, time_base);
        self.unbundler
            .input_context
            .seek(start_timestamp, ..start_timestamp)?;

        let expected_count = (end - start + 1) as usize;
        let mut frames = Vec::with_capacity(expected_count);
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
                    crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

                if current_frame_number >= start && current_frame_number <= end {
                    scaler.run(&decoded_frame, &mut rgb_frame)?;
                    frames.push(convert_frame_to_image(
                        &rgb_frame,
                        target_width,
                        target_height,
                    )?);
                }

                if current_frame_number > end {
                    return Ok(frames);
                }
            }
        }

        // Flush the decoder.
        decoder.send_eof()?;
        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            let pts = decoded_frame.pts().unwrap_or(0);
            let current_frame_number =
                crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

            if current_frame_number >= start && current_frame_number <= end {
                scaler.run(&decoded_frame, &mut rgb_frame)?;
                frames.push(convert_frame_to_image(
                    &rgb_frame,
                    target_width,
                    target_height,
                )?);
            }

            if current_frame_number > end {
                break;
            }
        }

        Ok(frames)
    }

    /// Extract frames at specific (possibly non-contiguous) frame numbers.
    ///
    /// Sorts the requested frame numbers and processes them in order to
    /// minimise seeks. Sequential runs are decoded without re-seeking.
    fn extract_specific_frames(
        &mut self,
        frame_numbers: &[u64],
        video_metadata: &VideoMetadata,
    ) -> Result<Vec<DynamicImage>, UnbundleError> {
        if frame_numbers.is_empty() {
            return Ok(Vec::new());
        }

        let video_stream_index = self
            .unbundler
            .video_stream_index
            .ok_or(UnbundleError::NoVideoStream)?;

        let target_width = video_metadata.width;
        let target_height = video_metadata.height;
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

        // Seek to the first requested frame.
        let first_timestamp = crate::utilities::frame_number_to_stream_timestamp(
            sorted_numbers[0],
            frames_per_second,
            time_base,
        );
        self.unbundler
            .input_context
            .seek(first_timestamp, ..first_timestamp)?;

        let mut frames = Vec::with_capacity(sorted_numbers.len());
        let mut target_index = 0;
        let mut decoded_frame = VideoFrame::empty();
        let mut rgb_frame = VideoFrame::empty();

        for (stream, packet) in self.unbundler.input_context.packets() {
            if target_index >= sorted_numbers.len() {
                break;
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
                    scaler.run(&decoded_frame, &mut rgb_frame)?;
                    frames.push(convert_frame_to_image(
                        &rgb_frame,
                        target_width,
                        target_height,
                    )?);
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
                    scaler.run(&decoded_frame, &mut rgb_frame)?;
                    frames.push(convert_frame_to_image(
                        &rgb_frame,
                        target_width,
                        target_height,
                    )?);
                    target_index += 1;
                }
            }
        }

        Ok(frames)
    }
}

/// Convert a scaled RGB24 video frame to an [`image::DynamicImage`].
fn convert_frame_to_image(
    rgb_frame: &VideoFrame,
    width: u32,
    height: u32,
) -> Result<DynamicImage, UnbundleError> {
    let buffer = crate::utilities::frame_to_rgb_buffer(rgb_frame, width, height);
    let rgb_image = RgbImage::from_raw(width, height, buffer).ok_or_else(|| {
        UnbundleError::VideoDecodeError(
            "Failed to construct RGB image from decoded frame data".to_string(),
        )
    })?;
    Ok(DynamicImage::ImageRgb8(rgb_image))
}
