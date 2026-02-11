//! Lazy, pull-based video frame iterator.
//!
//! [`FrameIterator`] implements [`Iterator`] and decodes frames on demand —
//! each call to [`next()`](Iterator::next) reads and decodes just enough
//! packets to produce the next requested frame. This avoids buffering the
//! entire frame set in memory.
//!
//! Create a `FrameIterator` via [`VideoExtractor::frame_iter`](crate::VideoExtractor).
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{FrameRange, MediaUnbundler};
//!
//! let mut unbundler = MediaUnbundler::open("input.mp4")?;
//! let iter = unbundler.video().frame_iter(FrameRange::Range(0, 9))?;
//!
//! for result in iter {
//!     let (frame_number, image) = result?;
//!     image.save(format!("frame_{frame_number}.png"))?;
//! }
//! # Ok::<(), unbundle::UnbundleError>(())
//! ```

use ffmpeg_next::{
    codec::context::Context as CodecContext,
    decoder::Video as VideoDecoder,
    Error as FfmpegError,
    frame::Video as VideoFrame,
    Packet,
    Rational,
    software::scaling::{Context as ScalingContext, Flags as ScalingFlags},
};
use image::{DynamicImage, GrayImage, RgbImage, RgbaImage};

use crate::config::{FrameOutputConfig, PixelFormat};
use crate::error::UnbundleError;
use crate::unbundler::MediaUnbundler;

/// A lazy iterator over decoded video frames.
///
/// Frames are decoded one at a time as [`next()`](Iterator::next) is called.
/// The iterator borrows the underlying [`MediaUnbundler`] mutably, so no other
/// extraction can happen while it is alive. Dropping the iterator releases the
/// borrow.
///
/// Created via [`VideoExtractor::frame_iter`](crate::VideoExtractor).
pub struct FrameIterator<'a> {
    unbundler: &'a mut MediaUnbundler,
    decoder: VideoDecoder,
    scaler: ScalingContext,
    video_stream_index: usize,
    /// Sorted, deduplicated frame numbers to yield.
    target_frames: Vec<u64>,
    /// Index into `target_frames` pointing to the next frame to yield.
    target_index: usize,
    time_base: Rational,
    fps: f64,
    output_config: FrameOutputConfig,
    target_width: u32,
    target_height: u32,
    decoded_frame: VideoFrame,
    scaled_frame: VideoFrame,
    eof_sent: bool,
    done: bool,
}

impl<'a> FrameIterator<'a> {
    /// Create a new iterator over the given frame numbers.
    ///
    /// `frame_numbers` must be **sorted and deduplicated**. The iterator
    /// seeks to the first requested frame and then decodes forward.
    pub(crate) fn new(
        unbundler: &'a mut MediaUnbundler,
        frame_numbers: Vec<u64>,
        output_config: FrameOutputConfig,
    ) -> Result<Self, UnbundleError> {
        let video_stream_index = unbundler
            .video_stream_index
            .ok_or(UnbundleError::NoVideoStream)?;

        let video_metadata = unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?;

        let fps = video_metadata.frames_per_second;
        let (target_width, target_height) =
            output_config.resolve_dimensions(video_metadata.width, video_metadata.height);
        let output_pixel = output_config.pixel_format.to_ffmpeg_pixel();

        let stream = unbundler
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

        // Seek to the first requested frame.
        if let Some(&first) = frame_numbers.first() {
            let first_ts = crate::utilities::frame_number_to_stream_timestamp(
                first, fps, time_base,
            );
            let _ = unbundler.input_context.seek(first_ts, ..first_ts);
        }

        Ok(Self {
            unbundler,
            decoder,
            scaler,
            video_stream_index,
            target_frames: frame_numbers,
            target_index: 0,
            time_base,
            fps,
            output_config,
            target_width,
            target_height,
            decoded_frame: VideoFrame::empty(),
            scaled_frame: VideoFrame::empty(),
            eof_sent: false,
            done: false,
        })
    }

    /// Scale and convert the current `decoded_frame` to a `DynamicImage`.
    fn convert_current_frame(&mut self) -> Result<DynamicImage, UnbundleError> {
        self.scaler
            .run(&self.decoded_frame, &mut self.scaled_frame)?;

        let width = self.target_width;
        let height = self.target_height;

        match self.output_config.pixel_format {
            PixelFormat::Rgb8 => {
                let buf =
                    crate::utilities::frame_to_buffer(&self.scaled_frame, width, height, 3);
                let img = RgbImage::from_raw(width, height, buf).ok_or_else(|| {
                    UnbundleError::VideoDecodeError(
                        "Failed to construct RGB image from decoded frame data".to_string(),
                    )
                })?;
                Ok(DynamicImage::ImageRgb8(img))
            }
            PixelFormat::Rgba8 => {
                let buf =
                    crate::utilities::frame_to_buffer(&self.scaled_frame, width, height, 4);
                let img = RgbaImage::from_raw(width, height, buf).ok_or_else(|| {
                    UnbundleError::VideoDecodeError(
                        "Failed to construct RGBA image from decoded frame data".to_string(),
                    )
                })?;
                Ok(DynamicImage::ImageRgba8(img))
            }
            PixelFormat::Gray8 => {
                let buf =
                    crate::utilities::frame_to_buffer(&self.scaled_frame, width, height, 1);
                let img = GrayImage::from_raw(width, height, buf).ok_or_else(|| {
                    UnbundleError::VideoDecodeError(
                        "Failed to construct grayscale image from decoded frame data".to_string(),
                    )
                })?;
                Ok(DynamicImage::ImageLuma8(img))
            }
        }
    }
}

impl Iterator for FrameIterator<'_> {
    type Item = Result<(u64, DynamicImage), UnbundleError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.target_index >= self.target_frames.len() {
            return None;
        }

        loop {
            // Try to receive a frame the decoder has already produced.
            if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
                let pts = self.decoded_frame.pts().unwrap_or(0);
                let current_frame =
                    crate::utilities::pts_to_frame_number(pts, self.time_base, self.fps);

                // Skip targets we have already passed.
                while self.target_index < self.target_frames.len()
                    && self.target_frames[self.target_index] < current_frame
                {
                    self.target_index += 1;
                }

                if self.target_index >= self.target_frames.len() {
                    self.done = true;
                    return None;
                }

                if current_frame == self.target_frames[self.target_index] {
                    match self.convert_current_frame() {
                        Ok(image) => {
                            let frame_num = current_frame;
                            self.target_index += 1;
                            return Some(Ok((frame_num, image)));
                        }
                        Err(e) => {
                            self.done = true;
                            return Some(Err(e));
                        }
                    }
                }

                // Frame doesn't match a target — keep receiving.
                continue;
            }

            // Decoder has no buffered frames. Feed it more packets.
            if self.eof_sent {
                // Already sent EOF and decoder is drained.
                self.done = true;
                return None;
            }

            let mut packet = Packet::empty();
            match packet.read(&mut self.unbundler.input_context) {
                Ok(()) => {
                    if packet.stream() == self.video_stream_index {
                        if let Err(e) = self.decoder.send_packet(&packet) {
                            self.done = true;
                            return Some(Err(UnbundleError::from(e)));
                        }
                    }
                    // Non-video packets are silently skipped.
                }
                Err(FfmpegError::Eof) => {
                    if let Err(e) = self.decoder.send_eof() {
                        self.done = true;
                        return Some(Err(UnbundleError::from(e)));
                    }
                    self.eof_sent = true;
                }
                Err(_) => {
                    // Non-fatal read error — try the next packet.
                }
            }
        }
    }
}
