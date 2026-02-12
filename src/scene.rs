//! Scene change detection.
//!
//! Uses FFmpeg's `scdet` filter to detect scene changes (shot boundaries)
//! in a video stream. Results are returned as timestamps and frame numbers.
//!
//! This module is available when the `scene` feature is enabled.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{MediaFile, UnbundleError};
//!
//! let mut unbundler = MediaFile::open("input.mp4")?;
//! let scenes = unbundler.video().detect_scenes(None)?;
//! for scene in &scenes {
//!     println!("Scene at {:?} (frame {}), score {:.2}",
//!         scene.timestamp, scene.frame_number, scene.score);
//! }
//! # Ok::<(), UnbundleError>(())
//! ```

use std::ffi::CStr;
use std::time::Duration;

use ffmpeg_next::{
    codec::context::Context as CodecContext, filter::Graph as FilterGraph,
    Error as FfmpegError, Packet,
    frame::Video as VideoFrame,
};
use ffmpeg_sys_next::AVPixelFormat;

use crate::{error::UnbundleError, metadata::VideoMetadata, unbundle::MediaFile};

/// A detected scene change.
///
/// Each instance marks the boundary between two shots in the video.
#[derive(Debug, Clone)]
pub struct SceneChange {
    /// Timestamp of the scene change.
    pub timestamp: Duration,
    /// Frame number at which the change was detected.
    pub frame_number: u64,
    /// Scene-change confidence score (typically 0.0–100.0).
    ///
    /// Higher values indicate a more obvious cut. The threshold used during
    /// detection determines the minimum score reported.
    pub score: f64,
}

/// Strategy used for scene detection.
///
/// `Full` uses FFmpeg's `scdet` filter and decodes frames.
/// `Keyframes` uses packet-level keyframes as scene boundaries (very fast).
/// `Auto` chooses a strategy based on stream size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SceneDetectionMode {
    /// Choose automatically: prefer keyframe-based detection on long videos,
    /// otherwise run full `scdet` analysis.
    #[default]
    Auto,
    /// Full decode + `scdet` filter.
    Full,
    /// Fast packet-level keyframe boundary detection.
    Keyframes,
}

/// Scene detection settings.
///
/// Controls the sensitivity of the scene-change detector. The default
/// threshold of 10.0 works well for most content.
#[derive(Debug, Clone)]
pub struct SceneDetectionOptions {
    /// Minimum score for a frame to be considered a scene change.
    ///
    /// Range 0.0–100.0. Lower values detect more (weaker) cuts; higher
    /// values only detect obvious hard cuts. Default: 10.0.
    pub threshold: f64,
    /// Scene detection strategy.
    pub mode: SceneDetectionMode,
    /// Optional maximum analysis duration from the start of the stream.
    ///
    /// When set, scene detection stops once decoded frame timestamps exceed
    /// this duration. This is useful to keep latency predictable on long
    /// videos.
    pub max_duration: Option<Duration>,
    /// Optional maximum number of detected scene changes.
    ///
    /// When set, detection returns as soon as this many scene changes are
    /// found.
    pub max_scene_changes: Option<usize>,
}

impl Default for SceneDetectionOptions {
    fn default() -> Self {
        Self {
            threshold: 10.0,
            mode: SceneDetectionMode::Auto,
            max_duration: None,
            max_scene_changes: None,
        }
    }
}

impl SceneDetectionOptions {
    /// Create a new scene detection configuration with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the minimum score required for scene changes.
    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }

    /// Set the scene detection strategy.
    pub fn mode(mut self, mode: SceneDetectionMode) -> Self {
        self.mode = mode;
        self
    }

    /// Limit analysis to the first `duration` of the video.
    pub fn max_duration(mut self, duration: Duration) -> Self {
        self.max_duration = Some(duration);
        self
    }

    /// Stop after detecting at most `max_changes` scene changes.
    pub fn max_scene_changes(mut self, max_changes: usize) -> Self {
        self.max_scene_changes = Some(max_changes);
        self
    }
}

/// Detect scene changes in the video stream.
///
/// This function is called internally by [`VideoHandle::detect_scenes`]
/// (and [`VideoHandle::detect_scenes_with_options`]).
pub(crate) fn detect_scenes_impl(
    unbundler: &mut MediaFile,
    video_metadata: &VideoMetadata,
    config: &SceneDetectionOptions,
    cancel_check: Option<&dyn Fn() -> bool>,
    stream_index: Option<usize>,
) -> Result<Vec<SceneChange>, UnbundleError> {
    let selected_mode = match config.mode {
        SceneDetectionMode::Auto => {
            // On long videos, packet-level keyframe analysis is dramatically
            // faster and usually sufficient for sampling or chapter-like splits.
            if video_metadata.frame_count > 6_000 && config.max_duration.is_none() {
                SceneDetectionMode::Keyframes
            } else {
                SceneDetectionMode::Full
            }
        }
        mode => mode,
    };

    if selected_mode == SceneDetectionMode::Keyframes {
        return detect_scenes_from_keyframes(
            unbundler,
            video_metadata,
            config,
            cancel_check,
            stream_index,
        );
    }

    let video_stream_index = stream_index
        .or(unbundler.video_stream_index)
        .ok_or(UnbundleError::NoVideoStream)?;

    log::debug!(
        "Detecting scenes (stream={}, threshold={})",
        video_stream_index,
        config.threshold
    );

    let stream = unbundler
        .input_context
        .stream(video_stream_index)
        .ok_or(UnbundleError::NoVideoStream)?;
    let time_base = stream.time_base();
    let codec_parameters = stream.parameters();
    let decoder_context = CodecContext::from_parameters(codec_parameters)?;
    let mut decoder = decoder_context.decoder().video()?;

    let frames_per_second = video_metadata.frames_per_second;
    let max_timestamp = config
        .max_duration
        .map(|duration| crate::conversion::duration_to_stream_timestamp(duration, time_base));

    let mut scenes = Vec::new();
    let mut decoded_frame = VideoFrame::empty();
    let mut filtered_frame = VideoFrame::empty();

    // Discover the actual decoded pixel format by decoding the first frame.
    // The decoder's reported format before decoding may differ from the
    // real output (e.g. codec parameters say YUYV422 but output is YUV420P).
    // We still probe to get a reasonable starting format for the buffer
    // filter, but a `format` filter in the chain normalises any mid-stream
    // pixel-format changes to YUV420P before they reach `scdet`.
    let mut actual_pix_fmt: Option<i32> = None;

    'probe: for (stream, packet) in unbundler.input_context.packets() {
        if stream.index() != video_stream_index {
            continue;
        }

        decoder
            .send_packet(&packet)
            .map_err(|e| UnbundleError::VideoDecodeError(e.to_string()))?;

        if decoder.receive_frame(&mut decoded_frame).is_ok() {
            actual_pix_fmt = Some(
                AVPixelFormat::from(decoded_frame.format()) as i32,
            );
            break 'probe;
        }
    }

    let pix_fmt = actual_pix_fmt
        .unwrap_or(AVPixelFormat::from(decoder.format()) as i32);

    // Read colorspace and color range from the probed frame so the buffer
    // filter matches the decoded frame properties exactly. We read the raw
    // AVFrame fields directly because the safe Rust enum accessors have the
    // same discriminant-mismatch problem as Pixel.
    let (color_space, color_range) = if actual_pix_fmt.is_some() {
        unsafe {
            let ptr = decoded_frame.as_ptr();
            ((*ptr).colorspace as i32, (*ptr).color_range as i32)
        }
    } else {
        (2, 0) // AVCOL_SPC_UNSPECIFIED, AVCOL_RANGE_UNSPECIFIED
    };

    // Build the filter graph: buffer → format → scdet → buffersink
    //
    // The `format` filter normalises all frames to YUV420P. This is
    // necessary because some decoders change their output pixel format
    // mid-stream (e.g. first frame as YUV422P, subsequent as YUV420P),
    // which would cause the filter chain to reject frames with a
    // "Changing video frame properties on the fly" error.
    let mut graph = FilterGraph::new();

    let buffer_args = format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect=1/1:colorspace={}:range={}",
        decoder.width(),
        decoder.height(),
        pix_fmt,
        time_base.numerator(),
        time_base.denominator(),
        color_space,
        color_range,
    );

    graph
        .add(
            &ffmpeg_next::filter::find("buffer").ok_or_else(|| {
                UnbundleError::VideoDecodeError("FFmpeg 'buffer' filter not found".to_string())
            })?,
            "in",
            &buffer_args,
        )
        .map_err(|e| {
            UnbundleError::VideoDecodeError(format!("Failed to add buffer filter: {e}"))
        })?;

    graph
        .add(
            &ffmpeg_next::filter::find("buffersink").ok_or_else(|| {
                UnbundleError::VideoDecodeError("FFmpeg 'buffersink' filter not found".to_string())
            })?,
            "out",
            "",
        )
        .map_err(|e| {
            UnbundleError::VideoDecodeError(format!("Failed to add buffersink filter: {e}"))
        })?;

    let scdet_spec = format!(
        "scale=320:-1,format=pix_fmts=yuv420p,scdet=threshold={}",
        config.threshold
    );
    graph
        .output("in", 0)
        .map_err(|e| UnbundleError::VideoDecodeError(format!("Filter graph output error: {e}")))?
        .input("out", 0)
        .map_err(|e| UnbundleError::VideoDecodeError(format!("Filter graph input error: {e}")))?
        .parse(&scdet_spec)
        .map_err(|e| UnbundleError::VideoDecodeError(format!("Filter graph parse error: {e}")))?;

    graph
        .validate()
        .map_err(|e| UnbundleError::VideoDecodeError(format!("Filter graph validation: {e}")))?;

    // Helper: feed a decoded frame through the filter graph and collect scenes.
    let mut feed_and_collect = |graph: &mut FilterGraph,
                                frame: &VideoFrame,
                                scenes: &mut Vec<SceneChange>|
     -> Result<(), UnbundleError> {
        graph
            .get("in")
            .ok_or_else(|| UnbundleError::VideoDecodeError("Filter 'in' not found".to_string()))?
            .source()
            .add(frame)
            .map_err(|e| UnbundleError::VideoDecodeError(format!("Failed to feed filter: {e}")))?;

        while graph
            .get("out")
            .ok_or_else(|| UnbundleError::VideoDecodeError("Filter 'out' not found".to_string()))?
            .sink()
            .frame(&mut filtered_frame)
            .is_ok()
        {
            let score = read_scdet_score(&filtered_frame);
            if let Some(score) = score.filter(|&s| s >= config.threshold) {
                let pts = filtered_frame.pts().unwrap_or(0);
                let timestamp =
                    Duration::from_secs_f64(crate::conversion::pts_to_seconds(pts, time_base));
                let frame_number =
                    crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);
                scenes.push(SceneChange {
                    timestamp,
                    frame_number,
                    score,
                });

                if config
                    .max_scene_changes
                    .is_some_and(|max_changes| scenes.len() >= max_changes)
                {
                    return Ok(());
                }
            }
        }
        Ok(())
    };

    // Feed the first frame we already decoded (still in decoded_frame).
    if actual_pix_fmt.is_some() {
        feed_and_collect(&mut graph, &decoded_frame, &mut scenes)?;

        // The decoder may still have buffered frames from the first packet.
        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            feed_and_collect(&mut graph, &decoded_frame, &mut scenes)?;
        }
    }

    // Process remaining packets.
    for (stream, packet) in unbundler.input_context.packets() {
        if let Some(check) = cancel_check {
            if check() {
                return Err(UnbundleError::Cancelled);
            }
        }

        if stream.index() != video_stream_index {
            continue;
        }

        if let Some(max_pts) = max_timestamp
            && packet.pts().is_some_and(|pts| pts > max_pts)
        {
            break;
        }

        decoder
            .send_packet(&packet)
            .map_err(|e| UnbundleError::VideoDecodeError(e.to_string()))?;

        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            if let Some(max_pts) = max_timestamp
                && decoded_frame.pts().is_some_and(|pts| pts > max_pts)
            {
                return Ok(scenes);
            }
            feed_and_collect(&mut graph, &decoded_frame, &mut scenes)?;
        }
    }

    // Flush the decoder.
    let _ = decoder.send_eof();
    while decoder.receive_frame(&mut decoded_frame).is_ok() {
        if let Some(max_pts) = max_timestamp
            && decoded_frame.pts().is_some_and(|pts| pts > max_pts)
        {
            break;
        }
        let _ = feed_and_collect(&mut graph, &decoded_frame, &mut scenes);
    }

    // Drain remaining filter output.
    while graph
        .get("out")
        .map(|mut f| f.sink().frame(&mut filtered_frame).is_ok())
        .unwrap_or(false)
    {
        let score = read_scdet_score(&filtered_frame);
        if let Some(score) = score.filter(|&s| s >= config.threshold) {
            let pts = filtered_frame.pts().unwrap_or(0);
            let timestamp =
                Duration::from_secs_f64(crate::conversion::pts_to_seconds(pts, time_base));
            let frame_number =
                crate::conversion::pts_to_frame_number(pts, time_base, frames_per_second);

            scenes.push(SceneChange {
                timestamp,
                frame_number,
                score,
            });

            if config
                .max_scene_changes
                .is_some_and(|max_changes| scenes.len() >= max_changes)
            {
                break;
            }
        }
    }

    Ok(scenes)
}

/// Fast scene boundary detection using packet keyframes only.
///
/// This avoids full-frame decode and is suitable for long videos where
/// approximate boundaries are acceptable.
fn detect_scenes_from_keyframes(
    unbundler: &mut MediaFile,
    video_metadata: &VideoMetadata,
    config: &SceneDetectionOptions,
    cancel_check: Option<&dyn Fn() -> bool>,
    stream_index: Option<usize>,
) -> Result<Vec<SceneChange>, UnbundleError> {
    let video_stream_index = stream_index
        .or(unbundler.video_stream_index)
        .ok_or(UnbundleError::NoVideoStream)?;

    log::debug!(
        "Detecting scenes from keyframes (stream={}, max_duration={:?}, max_scene_changes={:?})",
        video_stream_index,
        config.max_duration,
        config.max_scene_changes,
    );

    let time_base = unbundler
        .input_context
        .stream(video_stream_index)
        .ok_or(UnbundleError::NoVideoStream)?
        .time_base();

    let max_stream_timestamp = config
        .max_duration
        .map(|duration| crate::conversion::duration_to_stream_timestamp(duration, time_base));

    let mut scenes = Vec::new();
    let mut video_packet_number: u64 = 0;
    let mut packet = Packet::empty();

    loop {
        if let Some(check) = cancel_check
            && check()
        {
            return Err(UnbundleError::Cancelled);
        }

        match packet.read(&mut unbundler.input_context) {
            Ok(()) => {
                if packet.stream() as usize != video_stream_index {
                    continue;
                }

                if let Some(max_pts) = max_stream_timestamp
                    && packet.pts().is_some_and(|pts| pts > max_pts)
                {
                    break;
                }

                if packet.is_key() {
                    // Skip the very first key packet (start-of-stream marker).
                    if video_packet_number > 0 {
                        let pts = packet.pts().unwrap_or(0);
                        let timestamp = Duration::from_secs_f64(
                            crate::conversion::pts_to_seconds(pts, time_base).max(0.0),
                        );
                        let frame_number = crate::conversion::pts_to_frame_number(
                            pts,
                            time_base,
                            video_metadata.frames_per_second,
                        );

                        scenes.push(SceneChange {
                            timestamp,
                            frame_number,
                            // Sentinel score to indicate keyframe-derived boundary.
                            score: 100.0,
                        });

                        if config
                            .max_scene_changes
                            .is_some_and(|max| scenes.len() >= max)
                        {
                            break;
                        }
                    }
                }

                video_packet_number += 1;
            }
            Err(FfmpegError::Eof) => break,
            Err(error) => return Err(UnbundleError::from(error)),
        }
    }

    Ok(scenes)
}

/// Read the `lavfi.scd.score` metadata value from a filtered frame.
///
/// The `scdet` filter adds this key to frames where it detects a scene change.
/// Returns `None` for frames without the key (i.e. not a scene boundary).
fn read_scdet_score(frame: &VideoFrame) -> Option<f64> {
    // SAFETY: We access the frame's metadata dictionary via ffmpeg_sys_next
    // because ffmpeg-next's safe API does not expose per-frame metadata.
    unsafe {
        let frame_ptr = frame.as_ptr();
        if frame_ptr.is_null() {
            return None;
        }

        let metadata = (*frame_ptr).metadata;
        if metadata.is_null() {
            return None;
        }

        let key = c"lavfi.scd.score";
        let entry = ffmpeg_sys_next::av_dict_get(metadata, key.as_ptr(), std::ptr::null(), 0);

        if entry.is_null() {
            return None;
        }

        let value_ptr = (*entry).value;
        if value_ptr.is_null() {
            return None;
        }

        let value_cstr = CStr::from_ptr(value_ptr);
        value_cstr.to_str().ok()?.parse::<f64>().ok()
    }
}
