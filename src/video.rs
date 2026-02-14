//! Video frame extraction.
//!
//! This module provides [`VideoHandle`] for extracting still frames from
//! video files, and [`FrameRange`] for specifying which frames to extract.
//! Extracted frames are returned as [`image::DynamicImage`] values that can be
//! saved, manipulated, or converted to other formats.

use std::ffi::CString;
use std::path::Path;
use std::time::Duration;

use ffmpeg_next::{
    Rational,
    codec::Id,
    codec::context::Context as CodecContext,
    decoder::Video as VideoDecoder,
    filter::Graph as FilterGraph,
    format::Pixel,
    frame::Video as VideoFrame,
    packet::Mut as PacketMut,
    software::scaling::{Context as ScalingContext, Flags as ScalingFlags},
    util::picture::Type as PictureType,
};
use ffmpeg_sys_next::{AVFormatContext, AVPixelFormat, AVRational};
use image::{DynamicImage, GrayImage, RgbImage, RgbaImage};

#[cfg(feature = "gif")]
use crate::gif::GifOptions;
#[cfg(feature = "scene")]
use crate::scene::{SceneChange, SceneDetectionOptions};
#[cfg(feature = "async")]
use crate::stream::FrameStream;
use crate::{
    configuration::{ExtractOptions, FrameOutputOptions, PixelFormat},
    error::UnbundleError,
    keyframe::{GroupOfPicturesInfo, KeyFrameMetadata},
    metadata::VideoMetadata,
    progress::{OperationType, ProgressTracker},
    unbundle::MediaFile,
    variable_framerate::VariableFrameRateAnalysis,
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

/// Zero-copy view over a decoded frame's primary plane and metadata.
///
/// Provided to [`VideoHandle::for_each_raw_frame`] callbacks. The `data`
/// slice borrows decoder-owned memory and is only valid for the callback
/// invocation.
#[derive(Debug, Clone, Copy)]
pub struct RawFrameView<'a> {
    /// The zero-indexed frame number within the video.
    pub frame_number: u64,
    /// Presentation timestamp in stream time-base units, if available.
    pub pts: Option<i64>,
    /// Presentation timestamp converted to [`Duration`].
    pub timestamp: Duration,
    /// Decoded frame width.
    pub width: u32,
    /// Decoded frame height.
    pub height: u32,
    /// Line stride (bytes per row) for `data`.
    pub stride: usize,
    /// Pixel format of this decoded frame.
    pub pixel_format: Pixel,
    /// Whether this frame is a keyframe.
    pub is_keyframe: bool,
    /// Picture type (I/P/B/etc.).
    pub frame_type: FrameType,
    /// Borrowed bytes from plane 0. May include row padding.
    pub data: &'a [u8],
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
    /// Extract keyframes only.
    ///
    /// Keyframes are discovered from packet metadata (without full decode)
    /// and converted to frame numbers using stream timestamps.
    KeyframesOnly,
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

/// Chainable FFmpeg filter helper for [`VideoHandle`].
///
/// Build a filter graph incrementally with repeated
/// [`filter`](FilterChainHandle::filter) calls, then extract frames.
///
/// # Example
///
/// ```no_run
/// use unbundle::{MediaFile, UnbundleError};
///
/// let mut unbundler = MediaFile::open("input.mp4")?;
/// let image = unbundler
///     .video()
///     .filter("scale=1280:720")
///     .filter("eq=brightness=0.1")
///     .frame(0)?;
/// # Ok::<(), UnbundleError>(())
/// ```
pub struct FilterChainHandle<'a> {
    video_handle: VideoHandle<'a>,
    filters: Vec<String>,
}

impl<'a> FilterChainHandle<'a> {
    fn new(video_handle: VideoHandle<'a>) -> Self {
        Self {
            video_handle,
            filters: Vec::new(),
        }
    }

    fn combined_filter_spec(&self) -> Option<String> {
        if self.filters.is_empty() {
            None
        } else {
            Some(self.filters.join(","))
        }
    }

    /// Append a filter to the chain.
    ///
    /// Empty filters are ignored.
    #[must_use]
    pub fn filter(mut self, filter_spec: &str) -> Self {
        let spec = filter_spec.trim();
        if !spec.is_empty() {
            self.filters.push(spec.to_string());
        }
        self
    }

    /// Extract a single frame using the chained filters.
    pub fn frame(mut self, frame_number: u64) -> Result<DynamicImage, UnbundleError> {
        self.frame_with_options(frame_number, &ExtractOptions::default())
    }

    /// Extract a single frame with options using the chained filters.
    pub fn frame_with_options(
        &mut self,
        frame_number: u64,
        config: &ExtractOptions,
    ) -> Result<DynamicImage, UnbundleError> {
        if let Some(filter_spec) = self.combined_filter_spec() {
            self.video_handle
                .frame_with_filter_with_options(frame_number, &filter_spec, config)
        } else {
            self.video_handle.frame_with_options(frame_number, config)
        }
    }

    /// Extract a frame at a timestamp using the chained filters.
    pub fn frame_at(mut self, timestamp: Duration) -> Result<DynamicImage, UnbundleError> {
        self.frame_at_with_options(timestamp, &ExtractOptions::default())
    }

    /// Extract a frame at a timestamp with options using the chained filters.
    pub fn frame_at_with_options(
        &mut self,
        timestamp: Duration,
        config: &ExtractOptions,
    ) -> Result<DynamicImage, UnbundleError> {
        let duration = self.video_handle.unbundler.metadata.duration;
        if timestamp > duration {
            return Err(UnbundleError::InvalidTimestamp(timestamp));
        }

        let frames_per_second = self
            .video_handle
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .frames_per_second;

        let frame_number = crate::conversion::timestamp_to_frame_number(timestamp, frames_per_second);
        self.frame_with_options(frame_number, config)
    }

    /// Extract and save a frame using the chained filters.
    pub fn save_frame<P: AsRef<Path>>(
        &mut self,
        frame_number: u64,
        path: P,
    ) -> Result<(), UnbundleError> {
        let image = self.frame_with_options(frame_number, &ExtractOptions::default())?;
        image.save(path)?;
        Ok(())
    }

    /// Extract and save a frame at timestamp using the chained filters.
    pub fn save_frame_at<P: AsRef<Path>>(
        &mut self,
        timestamp: Duration,
        path: P,
    ) -> Result<(), UnbundleError> {
        let image = self.frame_at_with_options(timestamp, &ExtractOptions::default())?;
        image.save(path)?;
        Ok(())
    }
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

    /// Start a chainable FFmpeg filter pipeline.
    ///
    /// This is a convenience wrapper around
    /// [`frame_with_filter`](VideoHandle::frame_with_filter) for incremental
    /// filter construction.
    #[must_use]
    pub fn filter(self, filter_spec: &str) -> FilterChainHandle<'a> {
        FilterChainHandle::new(self).filter(filter_spec)
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

    /// Extract a single frame, process it through a custom FFmpeg filter graph,
    /// and return the filtered image.
    ///
    /// `filter_spec` uses standard FFmpeg filter syntax (for example,
    /// `"scale=320:240"`, `"hflip"`, `"eq=brightness=0.05"`).
    ///
    /// # Errors
    ///
    /// Same as [`frame`](VideoHandle::frame), plus
    /// [`UnbundleError::FilterGraphError`] if filter graph creation or
    /// execution fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let frame = unbundler
    ///     .video()
    ///     .frame_with_filter(0, "scale=320:240,eq=contrast=1.1")?;
    /// frame.save("filtered.png")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn frame_with_filter(
        &mut self,
        frame_number: u64,
        filter_spec: &str,
    ) -> Result<DynamicImage, UnbundleError> {
        self.frame_with_filter_with_options(frame_number, filter_spec, &ExtractOptions::default())
    }

    /// Extract a single frame with a custom FFmpeg filter graph and extraction
    /// options.
    ///
    /// Like [`frame_with_filter`](VideoHandle::frame_with_filter), but also
    /// respects output pixel format, target resolution, and cancellation from
    /// the provided [`ExtractOptions`].
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested, or
    /// any error from [`frame_with_filter`](VideoHandle::frame_with_filter).
    pub fn frame_with_filter_with_options(
        &mut self,
        frame_number: u64,
        filter_spec: &str,
        config: &ExtractOptions,
    ) -> Result<DynamicImage, UnbundleError> {
        if filter_spec.trim().is_empty() {
            return Err(UnbundleError::FilterGraphError(
                "Filter specification cannot be empty".to_string(),
            ));
        }

        let video_stream_index = self.resolve_video_stream_index()?;

        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?;

        let total_frames = video_metadata.frame_count;
        let frames_per_second = video_metadata.frames_per_second;
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
            "Extracting filtered frame {} (filter='{}', stream={})",
            frame_number,
            filter_spec,
            video_stream_index
        );

        let stream = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?;
        let time_base = stream.time_base();
        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let (mut decoder, hardware_active) = create_video_decoder(decoder_context, config)?;

        let seek_timestamp =
            crate::conversion::frame_number_to_seek_timestamp(frame_number, frames_per_second);

        self.unbundler
            .input_context
            .seek(seek_timestamp, ..seek_timestamp)?;

        let mut decoded_frame = VideoFrame::empty();

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

                if current_frame_number >= frame_number {
                    let transferred =
                        maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
                    let source = transferred.as_ref().unwrap_or(&decoded_frame);
                    let filtered = apply_filter_graph_to_frame(source, time_base, filter_spec)?;

                    let (target_width, target_height) = config
                        .frame_output
                        .resolve_dimensions(filtered.width(), filtered.height());

                    let mut scaler = ScalingContext::get(
                        filtered.format(),
                        filtered.width(),
                        filtered.height(),
                        output_pixel,
                        target_width,
                        target_height,
                        ScalingFlags::BILINEAR,
                    )?;

                    let mut scaled_frame = VideoFrame::empty();
                    scaler.run(&filtered, &mut scaled_frame)?;
                    return convert_frame_to_image(
                        &scaled_frame,
                        target_width,
                        target_height,
                        &config.frame_output,
                    );
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

            if current_frame_number >= frame_number {
                let transferred = maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
                let source = transferred.as_ref().unwrap_or(&decoded_frame);
                let filtered = apply_filter_graph_to_frame(source, time_base, filter_spec)?;

                let (target_width, target_height) = config
                    .frame_output
                    .resolve_dimensions(filtered.width(), filtered.height());

                let mut scaler = ScalingContext::get(
                    filtered.format(),
                    filtered.width(),
                    filtered.height(),
                    output_pixel,
                    target_width,
                    target_height,
                    ScalingFlags::BILINEAR,
                )?;

                let mut scaled_frame = VideoFrame::empty();
                scaler.run(&filtered, &mut scaled_frame)?;
                return convert_frame_to_image(
                    &scaled_frame,
                    target_width,
                    target_height,
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
            &mut |_frame_number, frame_image, info| {
                results.push((frame_image, info));
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

    // ── Stream copy (lossless) ─────────────────────────────────────────

    /// Copy the video stream verbatim to a file without re-encoding.
    ///
    /// Unlike frame extraction methods, this copies packets directly from the
    /// input stream, preserving the original codec and quality. The output
    /// container format is inferred from the file extension.
    ///
    /// This is equivalent to `ffmpeg -i input.mp4 -an -sn -c:v copy output.mp4`.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoVideoStream`] if no video stream exists.
    /// - [`UnbundleError::StreamCopyError`] if the output container does
    ///   not support the source codec.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// unbundler.video().stream_copy("output.mp4")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn stream_copy<P: AsRef<Path>>(&mut self, path: P) -> Result<(), UnbundleError> {
        self.copy_stream_to_file(path.as_ref(), None, None, None)
    }

    /// Copy a video segment verbatim to a file without re-encoding.
    ///
    /// Like [`stream_copy`](VideoHandle::stream_copy) but copies only
    /// packets between `start` and `end`. Because there is no re-encoding,
    /// the actual boundaries are aligned to packet/keyframe boundaries.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::InvalidRange`] if `start >= end`.
    /// - Plus any errors from [`stream_copy`](VideoHandle::stream_copy).
    pub fn stream_copy_range<P: AsRef<Path>>(
        &mut self,
        path: P,
        start: Duration,
        end: Duration,
    ) -> Result<(), UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        self.copy_stream_to_file(path.as_ref(), Some(start), Some(end), None)
    }

    /// Copy the video stream verbatim to a file with cancellation support.
    ///
    /// Like [`stream_copy`](VideoHandle::stream_copy) but accepts an
    /// [`ExtractOptions`] for cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested,
    /// or any error from [`stream_copy`](VideoHandle::stream_copy).
    pub fn stream_copy_with_options<P: AsRef<Path>>(
        &mut self,
        path: P,
        config: &ExtractOptions,
    ) -> Result<(), UnbundleError> {
        self.copy_stream_to_file(path.as_ref(), None, None, Some(config))
    }

    /// Copy a video segment verbatim to a file with cancellation support.
    ///
    /// Like [`stream_copy_range`](VideoHandle::stream_copy_range) but
    /// accepts an [`ExtractOptions`].
    pub fn stream_copy_range_with_options<P: AsRef<Path>>(
        &mut self,
        path: P,
        start: Duration,
        end: Duration,
        config: &ExtractOptions,
    ) -> Result<(), UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        self.copy_stream_to_file(path.as_ref(), Some(start), Some(end), Some(config))
    }

    /// Copy the video stream verbatim to memory without re-encoding.
    ///
    /// `container_format` is the FFmpeg short name for the output container
    /// (for example: `"matroska"`, `"mp4"`, `"mpegts"`).
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoVideoStream`] if no video stream exists.
    /// - [`UnbundleError::StreamCopyError`] if the container format is
    ///   invalid or does not support the source codec.
    pub fn stream_copy_to_memory(
        &mut self,
        container_format: &str,
    ) -> Result<Vec<u8>, UnbundleError> {
        self.copy_stream_to_memory(container_format, None, None, None)
    }

    /// Copy a video segment verbatim to memory without re-encoding.
    ///
    /// Like [`stream_copy_to_memory`](VideoHandle::stream_copy_to_memory) but
    /// copies only packets between `start` and `end`.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::InvalidRange`] if `start >= end`.
    /// - Plus any errors from [`stream_copy_to_memory`](VideoHandle::stream_copy_to_memory).
    pub fn stream_copy_range_to_memory(
        &mut self,
        container_format: &str,
        start: Duration,
        end: Duration,
    ) -> Result<Vec<u8>, UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        self.copy_stream_to_memory(container_format, Some(start), Some(end), None)
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

        self.dispatch_range(
            range,
            &video_metadata,
            config,
            &mut |frame_number, frame_image| {
                frames.push(frame_image);
                tracker.advance(Some(frame_number), None);
                Ok(())
            },
        )?;

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

        self.dispatch_range(
            range,
            &video_metadata,
            config,
            &mut |frame_number, frame_image| {
                callback(frame_number, frame_image)?;
                tracker.advance(Some(frame_number), None);
                Ok(())
            },
        )?;

        tracker.finish();
        Ok(())
    }

    /// Process decoded frames as zero-copy byte slices plus metadata.
    ///
    /// Unlike [`for_each_frame`](VideoHandle::for_each_frame), this avoids
    /// conversion to [`DynamicImage`]. The callback receives a borrowed
    /// [`RawFrameView`] valid for the duration of that callback call.
    pub fn for_each_raw_frame<F>(
        &mut self,
        range: FrameRange,
        callback: F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(RawFrameView<'_>) -> Result<(), UnbundleError>,
    {
        self.for_each_raw_frame_with_options(range, &ExtractOptions::default(), callback)
    }

    /// Process decoded frames as zero-copy byte slices plus metadata,
    /// with progress/cancellation support.
    pub fn for_each_raw_frame_with_options<F>(
        &mut self,
        range: FrameRange,
        config: &ExtractOptions,
        mut callback: F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(RawFrameView<'_>) -> Result<(), UnbundleError>,
    {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let video_stream_index = self.resolve_video_stream_index()?;
        let time_base = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?
            .time_base();

        let total =
            Self::estimate_frame_count(&range, &video_metadata, self.unbundler.metadata.duration);

        let mut tracker = ProgressTracker::new(
            config.progress.clone(),
            OperationType::FrameExtraction,
            total,
            config.batch_size,
        );

        self.dispatch_range_raw(
            range,
            &video_metadata,
            config,
            &mut |frame_number, frame| {
                let pts = frame.pts();
                let timestamp = Duration::from_secs_f64(
                    crate::conversion::pts_to_seconds(pts.unwrap_or(0), time_base).max(0.0),
                );

                let view = RawFrameView {
                    frame_number,
                    pts,
                    timestamp,
                    width: frame.width(),
                    height: frame.height(),
                    stride: frame.stride(0),
                    pixel_format: frame.format(),
                    is_keyframe: frame.is_key(),
                    frame_type: picture_type_to_frame_type(frame.kind()),
                    data: frame.data(0),
                };

                callback(view)?;
                tracker.advance(Some(frame_number), Some(timestamp));
                Ok(())
            },
        )?;

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

        let scene_config = config.unwrap_or_default();
        crate::scene::detect_scenes_impl(
            self.unbundler,
            &video_metadata,
            &scene_config,
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
        scene_config: Option<SceneDetectionOptions>,
        config: &ExtractOptions,
    ) -> Result<Vec<SceneChange>, UnbundleError> {
        let video_metadata = self
            .unbundler
            .metadata
            .video
            .as_ref()
            .ok_or(UnbundleError::NoVideoStream)?
            .clone();

        let scene_config = scene_config.unwrap_or_default();
        let cancel_check: Box<dyn Fn() -> bool> = Box::new(|| config.is_cancelled());
        crate::scene::detect_scenes_impl(
            self.unbundler,
            &video_metadata,
            &scene_config,
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

        let frame_output_config =
            gif_config.to_frame_output_config(video_metadata.width, video_metadata.height);
        let extraction_config = ExtractOptions::default().with_frame_output(frame_output_config);
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

        let frame_output_config =
            gif_config.to_frame_output_config(video_metadata.width, video_metadata.height);
        let extraction_config = ExtractOptions::default().with_frame_output(frame_output_config);
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
    /// let analysis = unbundler.video().analyze_variable_framerate()?;
    /// println!("VFR: {}, mean FPS: {:.2}", analysis.is_variable_frame_rate, analysis.mean_frames_per_second);
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

        let source = self.unbundler.source.clone();
        Ok(crate::stream::create_frame_stream(
            source, range, config, None,
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
    pub fn frame_iter(mut self, range: FrameRange) -> Result<FrameIterator<'a>, UnbundleError> {
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
        mut self,
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
        &mut self,
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
            FrameRange::KeyframesOnly => self.resolve_keyframe_numbers(video_metadata)?,
            FrameRange::Segments(segments) => Self::resolve_segments(&segments, video_metadata)?,
        };
        numbers.sort_unstable();
        numbers.dedup();
        Ok(numbers)
    }

    /// Resolve keyframes into sorted, deduplicated frame numbers.
    fn resolve_keyframe_numbers(
        &mut self,
        video_metadata: &VideoMetadata,
    ) -> Result<Vec<u64>, UnbundleError> {
        let video_stream_index = self.resolve_video_stream_index()?;
        let keyframes = crate::keyframe::analyze_group_of_pictures_impl(
            self.unbundler,
            video_stream_index,
        )?
        .keyframes;

        let mut numbers: Vec<u64> = keyframes
            .into_iter()
            .filter_map(|keyframe| {
                keyframe.timestamp.map(|timestamp| {
                    crate::conversion::timestamp_to_frame_number(
                        timestamp,
                        video_metadata.frames_per_second,
                    )
                })
            })
            .collect();

        if numbers.is_empty() {
            numbers.push(0);
        }

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
            &self.unbundler.source,
            &frame_numbers,
            &video_metadata,
            config,
        )?;

        Ok(results
            .into_iter()
            .map(|(_, frame_image)| frame_image)
            .collect())
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
                let frames_per_second = video_metadata.frames_per_second;
                let start_frame =
                    crate::conversion::timestamp_to_frame_number(*start_time, frames_per_second);
                let end_frame =
                    crate::conversion::timestamp_to_frame_number(*end_time, frames_per_second);
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
            FrameRange::KeyframesOnly => None,
            FrameRange::Segments(segments) => {
                let frames_per_second = video_metadata.frames_per_second;
                let total: u64 = segments
                    .iter()
                    .map(|(start, end)| {
                        let start_frame =
                            crate::conversion::timestamp_to_frame_number(*start, frames_per_second);
                        let end_frame =
                            crate::conversion::timestamp_to_frame_number(*end, frames_per_second);
                        end_frame.saturating_sub(start_frame) + 1
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
            FrameRange::KeyframesOnly => {
                let numbers = self.resolve_keyframe_numbers(video_metadata)?;
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
            FrameRange::KeyframesOnly => {
                let numbers = self.resolve_keyframe_numbers(video_metadata)?;
                self.process_specific_frames_and_metadata(&numbers, video_metadata, config, handler)
            }
            FrameRange::Segments(segments) => {
                let numbers = Self::resolve_segments(&segments, video_metadata)?;
                self.process_specific_frames_and_metadata(&numbers, video_metadata, config, handler)
            }
        }
    }

    /// Validate and dispatch a [`FrameRange`] for raw frame processing.
    fn dispatch_range_raw<F>(
        &mut self,
        range: FrameRange,
        video_metadata: &VideoMetadata,
        config: &ExtractOptions,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, &VideoFrame) -> Result<(), UnbundleError>,
    {
        match range {
            FrameRange::Range(start, end) => {
                if start > end {
                    return Err(UnbundleError::InvalidRange {
                        start: format!("frame {start}"),
                        end: format!("frame {end}"),
                    });
                }
                self.process_frame_range_raw(start, end, video_metadata, config, handler)
            }
            FrameRange::Interval(step) => {
                if step == 0 {
                    return Err(UnbundleError::InvalidInterval);
                }
                let total = video_metadata.frame_count;
                let numbers: Vec<u64> = (0..total).step_by(step as usize).collect();
                self.process_specific_frames_raw(&numbers, video_metadata, config, handler)
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
                self.process_frame_range_raw(start_frame, end_frame, video_metadata, config, handler)
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
                self.process_specific_frames_raw(&numbers, video_metadata, config, handler)
            }
            FrameRange::Specific(numbers) => {
                self.process_specific_frames_raw(&numbers, video_metadata, config, handler)
            }
            FrameRange::KeyframesOnly => {
                let numbers = self.resolve_keyframe_numbers(video_metadata)?;
                self.process_specific_frames_raw(&numbers, video_metadata, config, handler)
            }
            FrameRange::Segments(segments) => {
                let numbers = Self::resolve_segments(&segments, video_metadata)?;
                self.process_specific_frames_raw(&numbers, video_metadata, config, handler)
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
        let (mut decoder, hardware_active) = create_video_decoder(decoder_context, config)?;

        let mut scaler: Option<ScalingContext> = if hardware_active {
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
                    let transferred =
                        maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
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
                let transferred = maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
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
        let (mut decoder, hardware_active) = create_video_decoder(decoder_context, config)?;

        let mut scaler: Option<ScalingContext> = if hardware_active {
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
                    let transferred =
                        maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
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
                    let transferred =
                        maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
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

    /// Decode a contiguous frame range and pass raw decoded frames.
    fn process_frame_range_raw<F>(
        &mut self,
        start: u64,
        end: u64,
        video_metadata: &VideoMetadata,
        config: &ExtractOptions,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, &VideoFrame) -> Result<(), UnbundleError>,
    {
        let video_stream_index = self.resolve_video_stream_index()?;
        let frames_per_second = video_metadata.frames_per_second;

        let stream = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?;
        let time_base = stream.time_base();
        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let (mut decoder, hardware_active) = create_video_decoder(decoder_context, config)?;

        let seek_timestamp = crate::conversion::frame_number_to_seek_timestamp(start, frames_per_second);
        self.unbundler
            .input_context
            .seek(seek_timestamp, ..seek_timestamp)?;

        let mut decoded_frame = VideoFrame::empty();

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
                    let transferred = maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
                    if let Some(raw_frame) = transferred.as_ref() {
                        handler(current_frame_number, raw_frame)?;
                    } else {
                        handler(current_frame_number, &decoded_frame)?;
                    }
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
                let transferred = maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
                if let Some(raw_frame) = transferred.as_ref() {
                    handler(current_frame_number, raw_frame)?;
                } else {
                    handler(current_frame_number, &decoded_frame)?;
                }
            }

            if current_frame_number > end {
                break;
            }
        }

        Ok(())
    }

    /// Decode specific frames and pass raw decoded frames.
    fn process_specific_frames_raw<F>(
        &mut self,
        frame_numbers: &[u64],
        video_metadata: &VideoMetadata,
        config: &ExtractOptions,
        handler: &mut F,
    ) -> Result<(), UnbundleError>
    where
        F: FnMut(u64, &VideoFrame) -> Result<(), UnbundleError>,
    {
        if frame_numbers.is_empty() {
            return Ok(());
        }

        let video_stream_index = self.resolve_video_stream_index()?;
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
        let (mut decoder, hardware_active) = create_video_decoder(decoder_context, config)?;

        let seek_timestamp = crate::conversion::frame_number_to_seek_timestamp(
            sorted_numbers[0],
            frames_per_second,
        );
        self.unbundler
            .input_context
            .seek(seek_timestamp, ..seek_timestamp)?;

        let mut target_index = 0;
        let mut decoded_frame = VideoFrame::empty();

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
                    let transferred = maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
                    if let Some(raw_frame) = transferred.as_ref() {
                        handler(current_frame_number, raw_frame)?;
                    } else {
                        handler(current_frame_number, &decoded_frame)?;
                    }
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
                    let transferred = maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
                    if let Some(raw_frame) = transferred.as_ref() {
                        handler(current_frame_number, raw_frame)?;
                    } else {
                        handler(current_frame_number, &decoded_frame)?;
                    }
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
        let (mut decoder, hardware_active) = create_video_decoder(decoder_context, config)?;

        // Defer scaler creation when hardware accel is active — the software pixel
        // format is only known after the first frame transfer.
        let mut scaler: Option<ScalingContext> = if hardware_active {
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
                    let transferred =
                        maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
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
                let transferred = maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
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
        let (mut decoder, hardware_active) = create_video_decoder(decoder_context, config)?;

        let mut scaler: Option<ScalingContext> = if hardware_active {
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
                    let transferred =
                        maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
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
                    let transferred =
                        maybe_transfer_hardware_frame(&decoded_frame, hardware_active)?;
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

    // ── Stream copy (lossless) helpers ──────────────────────────────

    /// Copy the video stream verbatim to a file without decoding or
    /// re-encoding. Container format is inferred from the file extension.
    fn copy_stream_to_file(
        &mut self,
        path: &Path,
        start: Option<Duration>,
        end: Option<Duration>,
        config: Option<&ExtractOptions>,
    ) -> Result<(), UnbundleError> {
        let video_stream_index = self.resolve_video_stream_index()?;
        log::debug!(
            "Stream-copying video to file {:?} (stream={})",
            path,
            video_stream_index
        );

        let stream = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?;
        let input_time_base = stream.time_base();

        let mut output_context = ffmpeg_next::format::output(&path).map_err(|error| {
            UnbundleError::StreamCopyError(format!("Failed to create output: {error}"))
        })?;

        {
            let mut out_stream = output_context
                .add_stream(ffmpeg_next::encoder::find(Id::None))
                .map_err(|error| {
                    UnbundleError::StreamCopyError(format!("Failed to add stream: {error}"))
                })?;
            out_stream.set_parameters(stream.parameters());
            unsafe {
                (*out_stream.parameters().as_mut_ptr()).codec_tag = 0;
            }
        }

        output_context.write_header().map_err(|error| {
            UnbundleError::StreamCopyError(format!("Failed to write header: {error}"))
        })?;

        if let Some(start_time) = start {
            let seek_timestamp = crate::conversion::duration_to_seek_timestamp(start_time);
            self.unbundler
                .input_context
                .seek(seek_timestamp, ..seek_timestamp)?;
        }

        let end_stream_timestamp = end.map(|end_time| {
            crate::conversion::duration_to_stream_timestamp(end_time, input_time_base)
        });

        let mut tracker = config.map(|active_config| {
            ProgressTracker::new(
                active_config.progress.clone(),
                OperationType::StreamCopy,
                None,
                active_config.batch_size,
            )
        });

        let output_time_base = output_context.stream(0).unwrap().time_base();

        for (stream, mut packet) in self.unbundler.input_context.packets() {
            if let Some(active_config) = config
                && active_config.is_cancelled()
            {
                return Err(UnbundleError::Cancelled);
            }
            if stream.index() != video_stream_index {
                continue;
            }

            if let Some(end_timestamp) = end_stream_timestamp
                && let Some(pts) = packet.pts()
                && pts > end_timestamp
            {
                break;
            }

            packet.set_stream(0);
            packet.rescale_ts(input_time_base, output_time_base);
            packet.set_position(-1);
            packet
                .write_interleaved(&mut output_context)
                .map_err(|error| {
                    UnbundleError::StreamCopyError(format!("Failed to write packet: {error}"))
                })?;

            if let Some(active_tracker) = tracker.as_mut() {
                active_tracker.advance(None, None);
            }
        }

        if let Some(active_tracker) = tracker.as_mut() {
            active_tracker.finish();
        }

        output_context.write_trailer().map_err(|error| {
            UnbundleError::StreamCopyError(format!("Failed to write trailer: {error}"))
        })?;

        Ok(())
    }

    /// Copy the video stream verbatim to memory without decoding or
    /// re-encoding, using FFmpeg dynamic buffer I/O.
    fn copy_stream_to_memory(
        &mut self,
        container_format: &str,
        start: Option<Duration>,
        end: Option<Duration>,
        config: Option<&ExtractOptions>,
    ) -> Result<Vec<u8>, UnbundleError> {
        let video_stream_index = self.resolve_video_stream_index()?;
        log::debug!(
            "Stream-copying video to memory (format={}, stream={})",
            container_format,
            video_stream_index
        );

        let stream = self
            .unbundler
            .input_context
            .stream(video_stream_index)
            .ok_or(UnbundleError::NoVideoStream)?;
        let input_time_base = stream.time_base();
        let codec_parameters = stream.parameters();

        if let Some(start_time) = start {
            let seek_timestamp = crate::conversion::duration_to_seek_timestamp(start_time);
            self.unbundler
                .input_context
                .seek(seek_timestamp, ..seek_timestamp)?;
        }

        let end_stream_timestamp = end.map(|end_time| {
            crate::conversion::duration_to_stream_timestamp(end_time, input_time_base)
        });

        let mut tracker = config.map(|active_config| {
            ProgressTracker::new(
                active_config.progress.clone(),
                OperationType::StreamCopy,
                None,
                active_config.batch_size,
            )
        });

        unsafe {
            let container_name_c = CString::new(container_format).map_err(|error| {
                UnbundleError::StreamCopyError(format!("Invalid container format name: {error}"))
            })?;

            let mut output_format_context: *mut AVFormatContext = std::ptr::null_mut();
            let allocation_result = ffmpeg_sys_next::avformat_alloc_output_context2(
                &mut output_format_context,
                std::ptr::null_mut(),
                container_name_c.as_ptr(),
                std::ptr::null(),
            );
            if allocation_result < 0 || output_format_context.is_null() {
                return Err(UnbundleError::StreamCopyError(
                    "Failed to allocate output format context".to_string(),
                ));
            }

            let dynamic_buffer_result =
                ffmpeg_sys_next::avio_open_dyn_buf(&mut (*output_format_context).pb);
            if dynamic_buffer_result < 0 {
                ffmpeg_sys_next::avformat_free_context(output_format_context);
                return Err(UnbundleError::StreamCopyError(
                    "Failed to open dynamic buffer".to_string(),
                ));
            }

            let output_stream =
                ffmpeg_sys_next::avformat_new_stream(output_format_context, std::ptr::null());
            if output_stream.is_null() {
                let mut buffer_pointer: *mut u8 = std::ptr::null_mut();
                ffmpeg_sys_next::avio_close_dyn_buf(
                    (*output_format_context).pb,
                    &mut buffer_pointer,
                );
                if !buffer_pointer.is_null() {
                    ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                }
                (*output_format_context).pb = std::ptr::null_mut();
                ffmpeg_sys_next::avformat_free_context(output_format_context);
                return Err(UnbundleError::StreamCopyError(
                    "Failed to add output stream".to_string(),
                ));
            }

            ffmpeg_sys_next::avcodec_parameters_copy(
                (*output_stream).codecpar,
                codec_parameters.as_ptr(),
            );
            (*(*output_stream).codecpar).codec_tag = 0;

            (*output_stream).time_base = AVRational {
                num: input_time_base.numerator(),
                den: input_time_base.denominator(),
            };

            let write_header_result =
                ffmpeg_sys_next::avformat_write_header(output_format_context, std::ptr::null_mut());
            if write_header_result < 0 {
                let mut buffer_pointer: *mut u8 = std::ptr::null_mut();
                ffmpeg_sys_next::avio_close_dyn_buf(
                    (*output_format_context).pb,
                    &mut buffer_pointer,
                );
                if !buffer_pointer.is_null() {
                    ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                }
                (*output_format_context).pb = std::ptr::null_mut();
                ffmpeg_sys_next::avformat_free_context(output_format_context);
                return Err(UnbundleError::StreamCopyError(
                    "Failed to write output header".to_string(),
                ));
            }

            let output_time_base = Rational::new(
                (*output_stream).time_base.num,
                (*output_stream).time_base.den,
            );

            for (stream, mut packet) in self.unbundler.input_context.packets() {
                if let Some(active_config) = config
                    && active_config.is_cancelled()
                {
                    let mut buffer_pointer: *mut u8 = std::ptr::null_mut();
                    ffmpeg_sys_next::avio_close_dyn_buf(
                        (*output_format_context).pb,
                        &mut buffer_pointer,
                    );
                    if !buffer_pointer.is_null() {
                        ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                    }
                    (*output_format_context).pb = std::ptr::null_mut();
                    ffmpeg_sys_next::avformat_free_context(output_format_context);
                    return Err(UnbundleError::Cancelled);
                }

                if stream.index() != video_stream_index {
                    continue;
                }

                if let Some(end_timestamp) = end_stream_timestamp
                    && let Some(pts) = packet.pts()
                    && pts > end_timestamp
                {
                    break;
                }

                packet.set_stream(0);
                packet.rescale_ts(input_time_base, output_time_base);
                packet.set_position(-1);
                ffmpeg_sys_next::av_interleaved_write_frame(
                    output_format_context,
                    packet.as_mut_ptr(),
                );

                if let Some(active_tracker) = tracker.as_mut() {
                    active_tracker.advance(None, None);
                }
            }

            if let Some(active_tracker) = tracker.as_mut() {
                active_tracker.finish();
            }

            ffmpeg_sys_next::av_write_trailer(output_format_context);

            let mut buffer_pointer: *mut u8 = std::ptr::null_mut();
            let buffer_size = ffmpeg_sys_next::avio_close_dyn_buf(
                (*output_format_context).pb,
                &mut buffer_pointer,
            );

            let result_bytes = if buffer_size > 0 && !buffer_pointer.is_null() {
                std::slice::from_raw_parts(buffer_pointer, buffer_size as usize).to_vec()
            } else {
                Vec::new()
            };

            if !buffer_pointer.is_null() {
                ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
            }

            (*output_format_context).pb = std::ptr::null_mut();
            ffmpeg_sys_next::avformat_free_context(output_format_context);

            Ok(result_bytes)
        }
    }
}

/// Create a video decoder, optionally with hardware acceleration.
///
/// Returns `(decoder, hardware_active)` where `hardware_active` indicates
/// whether hardware decoding was successfully initialised.
fn create_video_decoder(
    codec_context: CodecContext,
    #[allow(unused_variables)] config: &ExtractOptions,
) -> Result<(VideoDecoder, bool), UnbundleError> {
    #[cfg(feature = "hardware")]
    {
        let setup = crate::hardware_acceleration::try_create_hardware_decoder(
            codec_context,
            config.hardware_acceleration,
        )?;
        Ok((setup.decoder, setup.hardware_active))
    }
    #[cfg(not(feature = "hardware"))]
    {
        let decoder = codec_context.decoder().video()?;
        Ok((decoder, false))
    }
}

/// If hardware decoding is active, transfer a decoded frame from GPU to
/// system memory.  Returns `Some(software_frame)` on successful transfer,
/// `None` when the frame is already in system memory or when hardware
/// acceleration is not enabled.
fn maybe_transfer_hardware_frame(
    #[allow(unused_variables)] frame: &VideoFrame,
    #[allow(unused_variables)] hardware_active: bool,
) -> Result<Option<VideoFrame>, UnbundleError> {
    #[cfg(feature = "hardware")]
    if hardware_active {
        match crate::hardware_acceleration::transfer_hardware_frame(frame) {
            Ok(software_frame) => return Ok(Some(software_frame)),
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

/// Apply a custom FFmpeg filter graph to a decoded frame.
///
/// The graph is built as: `buffer -> <filter_spec> -> buffersink`.
fn apply_filter_graph_to_frame(
    frame: &VideoFrame,
    time_base: Rational,
    filter_spec: &str,
) -> Result<VideoFrame, UnbundleError> {
    let mut graph = FilterGraph::new();

    let pixel_format = AVPixelFormat::from(frame.format()) as i32;
    let buffer_args = format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect=1/1",
        frame.width(),
        frame.height(),
        pixel_format,
        time_base.numerator(),
        time_base.denominator(),
    );

    graph
        .add(
            &ffmpeg_next::filter::find("buffer").ok_or_else(|| {
                UnbundleError::FilterGraphError("FFmpeg 'buffer' filter not found".to_string())
            })?,
            "in",
            &buffer_args,
        )
        .map_err(|error| {
            UnbundleError::FilterGraphError(format!("Failed to add buffer filter: {error}"))
        })?;

    graph
        .add(
            &ffmpeg_next::filter::find("buffersink").ok_or_else(|| {
                UnbundleError::FilterGraphError("FFmpeg 'buffersink' filter not found".to_string())
            })?,
            "out",
            "",
        )
        .map_err(|error| {
            UnbundleError::FilterGraphError(format!("Failed to add buffersink filter: {error}"))
        })?;

    graph
        .output("in", 0)
        .map_err(|error| {
            UnbundleError::FilterGraphError(format!("Filter graph output error: {error}"))
        })?
        .input("out", 0)
        .map_err(|error| {
            UnbundleError::FilterGraphError(format!("Filter graph input error: {error}"))
        })?
        .parse(filter_spec)
        .map_err(|error| {
            UnbundleError::FilterGraphError(format!("Filter graph parse error: {error}"))
        })?;

    graph.validate().map_err(|error| {
        UnbundleError::FilterGraphError(format!("Filter graph validation error: {error}"))
    })?;

    graph
        .get("in")
        .ok_or_else(|| UnbundleError::FilterGraphError("Filter 'in' not found".to_string()))?
        .source()
        .add(frame)
        .map_err(|error| {
            UnbundleError::FilterGraphError(format!("Failed to feed filter graph: {error}"))
        })?;

    let mut filtered_frame = VideoFrame::empty();
    graph
        .get("out")
        .ok_or_else(|| UnbundleError::FilterGraphError("Filter 'out' not found".to_string()))?
        .sink()
        .frame(&mut filtered_frame)
        .map_err(|error| {
            UnbundleError::FilterGraphError(format!(
                "Filter graph did not produce an output frame: {error}"
            ))
        })?;

    Ok(filtered_frame)
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
