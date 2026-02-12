//! Container format conversion (remuxing).
//!
//! This module provides [`Remuxer`] for converting media files between
//! container formats without re-encoding. This is equivalent to
//! `ffmpeg -i input.mkv -c copy output.mp4`.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{Remuxer, UnbundleError};
//!
//! // Convert MKV to MP4 without re-encoding
//! Remuxer::new("input.mkv", "output.mp4")?.run()?;
//! # Ok::<(), UnbundleError>(())
//! ```

use std::path::{Path, PathBuf};

use ffmpeg_next::{codec::Id, media::Type};

use crate::configuration::ExtractOptions;
use crate::error::UnbundleError;
use crate::progress::{OperationType, ProgressTracker};

/// Lossless container format converter.
///
/// Copies all stream data from the input file to a new output container
/// without re-encoding audio, video, or subtitle tracks. The output format
/// is inferred from the file extension.
///
/// # Supported Conversions
///
/// Any combination of containers supported by the FFmpeg build is possible
/// (e.g. MKV → MP4, AVI → MKV, MOV → WebM), provided the output container
/// supports the codecs present in the input.
///
/// # Example
///
/// ```no_run
/// use unbundle::{Remuxer, UnbundleError};
///
/// Remuxer::new("input.mkv", "output.mp4")?
///     .exclude_subtitles()
///     .run()?;
/// # Ok::<(), UnbundleError>(())
/// ```
pub struct Remuxer {
    input_path: PathBuf,
    output_path: PathBuf,
    copy_video: bool,
    copy_audio: bool,
    copy_subtitles: bool,
}

impl Remuxer {
    /// Create a new remuxer from an input to an output file.
    ///
    /// The output container format is inferred from the file extension.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::FileOpen`] if the input file cannot be opened.
    pub fn new<P1: AsRef<Path>, P2: AsRef<Path>>(
        input: P1,
        output: P2,
    ) -> Result<Self, UnbundleError> {
        let input_path = input.as_ref().to_path_buf();
        let output_path = output.as_ref().to_path_buf();

        // Validate that FFmpeg is initialised and the input exists.
        ffmpeg_next::init().map_err(|e| UnbundleError::FileOpen {
            path: input_path.clone(),
            reason: format!("FFmpeg initialisation failed: {e}"),
        })?;

        if !input_path.exists() {
            return Err(UnbundleError::FileOpen {
                path: input_path,
                reason: "File does not exist".to_string(),
            });
        }

        Ok(Self {
            input_path,
            output_path,
            copy_video: true,
            copy_audio: true,
            copy_subtitles: true,
        })
    }

    /// Exclude video streams from the output.
    #[must_use]
    pub fn exclude_video(mut self) -> Self {
        self.copy_video = false;
        self
    }

    /// Exclude audio streams from the output.
    #[must_use]
    pub fn exclude_audio(mut self) -> Self {
        self.copy_audio = false;
        self
    }

    /// Exclude subtitle streams from the output.
    #[must_use]
    pub fn exclude_subtitles(mut self) -> Self {
        self.copy_subtitles = false;
        self
    }

    /// Execute the remuxing operation.
    ///
    /// Reads all packets from the input, remaps stream indices, and writes
    /// them to the output container. No re-encoding is performed.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::FileOpen`] if the output cannot be created,
    /// or [`UnbundleError::FfmpegError`] if remuxing fails.
    pub fn run(&self) -> Result<(), UnbundleError> {
        self.run_with_options(&ExtractOptions::default())
    }

    /// Execute the remuxing operation with progress and cancellation support.
    ///
    /// Like [`run`](Remuxer::run) but accepts an [`ExtractOptions`] for
    /// progress callbacks and cooperative cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested,
    /// or any error from [`run`](Remuxer::run).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::sync::Arc;
    ///
    /// use unbundle::{ExtractOptions, ProgressCallback, ProgressInfo, Remuxer, UnbundleError};
    ///
    /// struct PrintProgress;
    /// impl ProgressCallback for PrintProgress {
    ///     fn on_progress(&self, info: &ProgressInfo) {
    ///         println!("Remuxed {} packets", info.current);
    ///     }
    /// }
    ///
    /// Remuxer::new("input.mkv", "output.mp4")?
    ///     .run_with_options(
    ///         &ExtractOptions::new().with_progress(Arc::new(PrintProgress)),
    ///     )?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn run_with_options(&self, config: &ExtractOptions) -> Result<(), UnbundleError> {
        log::info!(
            "Remuxing {} → {} (video={}, audio={}, subtitles={})",
            self.input_path.display(),
            self.output_path.display(),
            self.copy_video,
            self.copy_audio,
            self.copy_subtitles,
        );
        let mut input_context =
            ffmpeg_next::format::input(&self.input_path).map_err(|e| UnbundleError::FileOpen {
                path: self.input_path.clone(),
                reason: e.to_string(),
            })?;

        let mut output_context = ffmpeg_next::format::output(&self.output_path).map_err(|e| {
            UnbundleError::FileOpen {
                path: self.output_path.clone(),
                reason: format!("Failed to create output: {e}"),
            }
        })?;

        // Build stream mapping: input_stream_index → output_stream_index.
        // Streams that are excluded get None.
        let mut stream_map: Vec<Option<usize>> = Vec::new();
        let mut output_stream_count: usize = 0;

        for stream in input_context.streams() {
            let medium = stream.parameters().medium();
            let include = match medium {
                Type::Video => self.copy_video,
                Type::Audio => self.copy_audio,
                Type::Subtitle => self.copy_subtitles,
                _ => false,
            };

            if include {
                // Add a corresponding output stream.
                let mut out_stream =
                    output_context.add_stream(ffmpeg_next::encoder::find(Id::None))?;
                out_stream.set_parameters(stream.parameters());
                // Reset codec tag to let the muxer choose.
                unsafe {
                    (*out_stream.parameters().as_mut_ptr()).codec_tag = 0;
                }
                stream_map.push(Some(output_stream_count));
                output_stream_count += 1;
            } else {
                stream_map.push(None);
            }
        }

        output_context.write_header()?;

        // Estimate total packets from the input duration (rough approximation).
        let total_packets: Option<u64> = None;
        let mut tracker = ProgressTracker::new(
            config.progress.clone(),
            OperationType::Remuxing,
            total_packets,
            config.batch_size,
        );

        // Copy packets, remapping stream indices.
        for (stream, mut packet) in input_context.packets() {
            if config.is_cancelled() {
                return Err(UnbundleError::Cancelled);
            }

            let input_idx = stream.index();
            let Some(output_idx) = stream_map.get(input_idx).copied().flatten() else {
                continue;
            };

            let input_time_base = stream.time_base();
            let output_time_base = output_context.stream(output_idx).unwrap().time_base();

            packet.set_stream(output_idx);
            packet.rescale_ts(input_time_base, output_time_base);
            packet.set_position(-1);
            packet.write_interleaved(&mut output_context)?;

            tracker.advance(None, None);
        }

        tracker.finish();

        output_context.write_trailer()?;
        Ok(())
    }
}
