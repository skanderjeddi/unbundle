//! Scene change detection.
//!
//! Uses FFmpeg's `scdet` filter to detect scene changes (shot boundaries)
//! in a video stream. Results are returned as timestamps and frame numbers.
//!
//! This module is available when the `scene-detection` feature is enabled.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::MediaUnbundler;
//!
//! let mut unbundler = MediaUnbundler::open("input.mp4")?;
//! let scenes = unbundler.video().detect_scenes(None)?;
//! for scene in &scenes {
//!     println!("Scene at {:?} (frame {}), score {:.2}",
//!         scene.timestamp, scene.frame_number, scene.score);
//! }
//! # Ok::<(), unbundle::UnbundleError>(())
//! ```

use std::ffi::CStr;
use std::time::Duration;

use ffmpeg_next::{
    codec::context::Context as CodecContext,
    filter::Graph as FilterGraph,
    frame::Video as VideoFrame,
};

use crate::{error::UnbundleError, metadata::VideoMetadata, unbundler::MediaUnbundler};

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

/// Scene detection settings.
///
/// Controls the sensitivity of the scene-change detector. The default
/// threshold of 10.0 works well for most content.
#[derive(Debug, Clone)]
pub struct SceneDetectionConfig {
    /// Minimum score for a frame to be considered a scene change.
    ///
    /// Range 0.0–100.0. Lower values detect more (weaker) cuts; higher
    /// values only detect obvious hard cuts. Default: 10.0.
    pub threshold: f64,
}

impl Default for SceneDetectionConfig {
    fn default() -> Self {
        Self { threshold: 10.0 }
    }
}

/// Detect scene changes in the video stream.
///
/// This function is called internally by [`VideoExtractor::detect_scenes`]
/// (and [`VideoExtractor::detect_scenes_with_config`]).
pub(crate) fn detect_scenes_impl(
    unbundler: &mut MediaUnbundler,
    video_metadata: &VideoMetadata,
    config: &SceneDetectionConfig,
    cancel_check: Option<&dyn Fn() -> bool>,
) -> Result<Vec<SceneChange>, UnbundleError> {
    let video_stream_index = unbundler
        .video_stream_index
        .ok_or(UnbundleError::NoVideoStream)?;

    let stream = unbundler
        .input_context
        .stream(video_stream_index)
        .ok_or(UnbundleError::NoVideoStream)?;
    let time_base = stream.time_base();
    let codec_parameters = stream.parameters();
    let decoder_context = CodecContext::from_parameters(codec_parameters)?;
    let mut decoder = decoder_context.decoder().video()?;

    let frames_per_second = video_metadata.frames_per_second;

    let mut scenes = Vec::new();
    let mut decoded_frame = VideoFrame::empty();
    let mut filtered_frame = VideoFrame::empty();

    // Discover the actual decoded pixel format by decoding the first frame.
    // The decoder's reported format before decoding may differ from the
    // real output (e.g. codec parameters say YUYV422 but output is YUV420P).
    let mut actual_pix_fmt: Option<i32> = None;

    'probe: for (stream, packet) in unbundler.input_context.packets() {
        if stream.index() != video_stream_index {
            continue;
        }

        decoder
            .send_packet(&packet)
            .map_err(|e| UnbundleError::VideoDecodeError(e.to_string()))?;

        if decoder.receive_frame(&mut decoded_frame).is_ok() {
            actual_pix_fmt = Some(decoded_frame.format() as i32);
            break 'probe;
        }
    }

    let pix_fmt = actual_pix_fmt.unwrap_or(decoder.format() as i32);

    // Build the filter graph: buffer → scdet → buffersink
    let mut graph = FilterGraph::new();

    let buffer_args = format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect=1/1",
        decoder.width(),
        decoder.height(),
        pix_fmt,
        time_base.numerator(),
        time_base.denominator(),
    );

    graph
        .add(&ffmpeg_next::filter::find("buffer").ok_or_else(|| {
            UnbundleError::VideoDecodeError("FFmpeg 'buffer' filter not found".to_string())
        })?, "in", &buffer_args)
        .map_err(|e| UnbundleError::VideoDecodeError(format!("Failed to add buffer filter: {e}")))?;

    graph
        .add(&ffmpeg_next::filter::find("buffersink").ok_or_else(|| {
            UnbundleError::VideoDecodeError("FFmpeg 'buffersink' filter not found".to_string())
        })?, "out", "")
        .map_err(|e| {
            UnbundleError::VideoDecodeError(format!("Failed to add buffersink filter: {e}"))
        })?;

    let scdet_spec = format!("scdet=threshold={}", config.threshold);
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
    let mut feed_and_collect =
        |graph: &mut FilterGraph,
         frame: &VideoFrame,
         scenes: &mut Vec<SceneChange>|
         -> Result<(), UnbundleError> {
            graph
                .get("in")
                .ok_or_else(|| {
                    UnbundleError::VideoDecodeError("Filter 'in' not found".to_string())
                })?
                .source()
                .add(frame)
                .map_err(|e| {
                    UnbundleError::VideoDecodeError(format!("Failed to feed filter: {e}"))
                })?;

            while graph
                .get("out")
                .ok_or_else(|| {
                    UnbundleError::VideoDecodeError("Filter 'out' not found".to_string())
                })?
                .sink()
                .frame(&mut filtered_frame)
                .is_ok()
            {
                let score = read_scdet_score(&filtered_frame);
                if let Some(score) = score.filter(|&s| s >= config.threshold) {
                    let pts = filtered_frame.pts().unwrap_or(0);
                    let timestamp = Duration::from_secs_f64(
                        crate::utilities::pts_to_seconds(pts, time_base),
                    );
                    let frame_number = crate::utilities::pts_to_frame_number(
                        pts,
                        time_base,
                        frames_per_second,
                    );
                    scenes.push(SceneChange {
                        timestamp,
                        frame_number,
                        score,
                    });
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

        decoder
            .send_packet(&packet)
            .map_err(|e| UnbundleError::VideoDecodeError(e.to_string()))?;

        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            feed_and_collect(&mut graph, &decoded_frame, &mut scenes)?;
        }
    }

    // Flush the decoder.
    let _ = decoder.send_eof();
    while decoder.receive_frame(&mut decoded_frame).is_ok() {
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
                Duration::from_secs_f64(crate::utilities::pts_to_seconds(pts, time_base));
            let frame_number =
                crate::utilities::pts_to_frame_number(pts, time_base, frames_per_second);

            scenes.push(SceneChange {
                timestamp,
                frame_number,
                score,
            });
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
        let entry = ffmpeg_sys_next::av_dict_get(
            metadata,
            key.as_ptr(),
            std::ptr::null(),
            0,
        );

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
