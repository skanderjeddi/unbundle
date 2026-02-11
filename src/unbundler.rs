//! Core [`MediaUnbundler`] implementation.
//!
//! `MediaUnbundler` is the main entry point for the crate. It opens a media
//! file, extracts and caches metadata, and provides access to
//! [`VideoExtractor`](crate::video::VideoExtractor) and
//! [`AudioExtractor`](crate::audio::AudioExtractor) for frame and audio
//! extraction respectively.

use std::{
    fmt::{Debug, Formatter, Result as FmtResult},
    path::{Path, PathBuf},
    time::Duration,
};

use ffmpeg_next::{codec::context::Context as CodecContext, format::context::Input, media::Type};

use crate::{
    audio::AudioExtractor,
    error::UnbundleError,
    metadata::{AudioMetadata, MediaMetadata, VideoMetadata},
    video::VideoExtractor,
};

/// Main struct for unbundling media files.
///
/// Created via [`MediaUnbundler::open`], this struct holds the demuxer context
/// and cached metadata. Use [`video()`](MediaUnbundler::video) and
/// [`audio()`](MediaUnbundler::audio) to obtain extractors for frames and
/// audio respectively.
///
/// # Example
///
/// ```no_run
/// use unbundle::MediaUnbundler;
///
/// let mut unbundler = MediaUnbundler::open("input.mp4").unwrap();
/// let metadata = unbundler.metadata();
/// println!("Duration: {:?}", metadata.duration);
///
/// // Extract a single frame
/// let frame = unbundler.video().frame(0).unwrap();
/// frame.save("first_frame.png").unwrap();
/// ```
pub struct MediaUnbundler {
    /// The opened FFmpeg input (demuxer) context.
    pub(crate) input_context: Input,
    /// Cached metadata extracted at open time.
    pub(crate) metadata: MediaMetadata,
    /// Index of the best video stream, if one exists.
    pub(crate) video_stream_index: Option<usize>,
    /// Index of the best audio stream, if one exists.
    pub(crate) audio_stream_index: Option<usize>,
    /// Path to the opened media file (kept for error messages).
    #[allow(dead_code)]
    pub(crate) file_path: PathBuf,
}

impl Debug for MediaUnbundler {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("MediaUnbundler")
            .field("metadata", &self.metadata)
            .field("video_stream_index", &self.video_stream_index)
            .field("audio_stream_index", &self.audio_stream_index)
            .field("file_path", &self.file_path)
            .finish_non_exhaustive()
    }
}

impl MediaUnbundler {
    /// Open a media file for extraction.
    ///
    /// Initializes FFmpeg (idempotent), opens the file, locates best video and
    /// audio streams, and caches their metadata.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::FileOpen`] if the file cannot be opened or has
    /// no recognisable media streams.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::MediaUnbundler;
    ///
    /// let unbundler = MediaUnbundler::open("video.mp4")?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, UnbundleError> {
        let path = path.as_ref();
        let canonical_path = path.to_path_buf();

        // Initialise ffmpeg (safe to call multiple times).
        ffmpeg_next::init().map_err(|error| UnbundleError::FileOpen {
            path: canonical_path.clone(),
            reason: format!("FFmpeg initialisation failed: {error}"),
        })?;

        // Open the media file.
        let input_context =
            ffmpeg_next::format::input(&path).map_err(|error| UnbundleError::FileOpen {
                path: canonical_path.clone(),
                reason: error.to_string(),
            })?;

        // Locate best video and audio streams.
        let video_stream_index = input_context
            .streams()
            .best(Type::Video)
            .map(|stream| stream.index());

        let audio_stream_index = input_context
            .streams()
            .best(Type::Audio)
            .map(|stream| stream.index());

        // Extract container-level duration.
        let duration_microseconds = input_context.duration();
        let duration = if duration_microseconds > 0 {
            Duration::from_micros(duration_microseconds as u64)
        } else {
            Duration::ZERO
        };

        // Extract container format name.
        let format = input_context.format().name().to_string();

        // Extract video metadata.
        let video_metadata = if let Some(index) = video_stream_index {
            let stream = input_context.stream(index).unwrap();
            let codec_parameters = stream.parameters();
            let decoder_context =
                CodecContext::from_parameters(codec_parameters).map_err(|error| {
                    UnbundleError::FileOpen {
                        path: canonical_path.clone(),
                        reason: format!("Failed to read video codec parameters: {error}"),
                    }
                })?;
            let video_decoder =
                decoder_context
                    .decoder()
                    .video()
                    .map_err(|error| UnbundleError::FileOpen {
                        path: canonical_path.clone(),
                        reason: format!("Failed to create video decoder: {error}"),
                    })?;

            let width = video_decoder.width();
            let height = video_decoder.height();

            // Compute frames per second from the stream's average frame rate.
            let frame_rate = stream.avg_frame_rate();
            let frames_per_second = if frame_rate.denominator() != 0 {
                frame_rate.numerator() as f64 / frame_rate.denominator() as f64
            } else {
                // Fallback: try the stream's rate field.
                let rate = stream.rate();
                if rate.denominator() != 0 {
                    rate.numerator() as f64 / rate.denominator() as f64
                } else {
                    0.0
                }
            };

            let frame_count = if frames_per_second > 0.0 {
                (duration.as_secs_f64() * frames_per_second) as u64
            } else {
                0
            };

            let codec_name = video_decoder
                .codec()
                .map(|codec| codec.name().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            Some(VideoMetadata {
                width,
                height,
                frames_per_second,
                frame_count,
                codec: codec_name,
            })
        } else {
            None
        };

        // Extract audio metadata.
        let audio_metadata = if let Some(index) = audio_stream_index {
            let stream = input_context.stream(index).unwrap();
            let codec_parameters = stream.parameters();
            let decoder_context =
                CodecContext::from_parameters(codec_parameters).map_err(|error| {
                    UnbundleError::FileOpen {
                        path: canonical_path.clone(),
                        reason: format!("Failed to read audio codec parameters: {error}"),
                    }
                })?;
            let audio_decoder =
                decoder_context
                    .decoder()
                    .audio()
                    .map_err(|error| UnbundleError::FileOpen {
                        path: canonical_path.clone(),
                        reason: format!("Failed to create audio decoder: {error}"),
                    })?;

            let sample_rate = audio_decoder.rate();
            let channels = audio_decoder.channels();
            let bit_rate = audio_decoder.bit_rate() as u64;

            let codec_name = audio_decoder
                .codec()
                .map(|codec| codec.name().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            Some(AudioMetadata {
                sample_rate,
                channels,
                codec: codec_name,
                bit_rate,
            })
        } else {
            None
        };

        let metadata = MediaMetadata {
            video: video_metadata,
            audio: audio_metadata,
            duration,
            format,
        };

        Ok(Self {
            input_context,
            metadata,
            video_stream_index,
            audio_stream_index,
            file_path: canonical_path,
        })
    }

    /// Get a reference to the cached media metadata.
    ///
    /// Metadata is extracted once during [`open`](MediaUnbundler::open) and
    /// does not require additional decoding.
    pub fn metadata(&self) -> &MediaMetadata {
        &self.metadata
    }

    /// Obtain a [`VideoExtractor`] for extracting video frames.
    ///
    /// The returned extractor borrows this unbundler mutably, so you cannot
    /// hold extractors for both video and audio simultaneously.
    pub fn video(&mut self) -> VideoExtractor<'_> {
        VideoExtractor { unbundler: self }
    }

    /// Obtain an [`AudioExtractor`] for extracting audio data.
    ///
    /// The returned extractor borrows this unbundler mutably, so you cannot
    /// hold extractors for both video and audio simultaneously.
    pub fn audio(&mut self) -> AudioExtractor<'_> {
        AudioExtractor { unbundler: self }
    }
}
