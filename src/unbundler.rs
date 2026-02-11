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
    metadata::{AudioMetadata, ChapterMetadata, MediaMetadata, SubtitleMetadata, VideoMetadata},
    subtitle::SubtitleExtractor,
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
    /// Indices of all audio streams, ordered by track number.
    pub(crate) audio_stream_indices: Vec<usize>,
    /// Index of the best subtitle stream, if one exists.
    pub(crate) subtitle_stream_index: Option<usize>,
    /// Indices of all subtitle streams, ordered by track number.
    pub(crate) subtitle_stream_indices: Vec<usize>,
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
            .field("audio_stream_indices", &self.audio_stream_indices)
            .field("subtitle_stream_index", &self.subtitle_stream_index)
            .field("subtitle_stream_indices", &self.subtitle_stream_indices)
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

        // Extract audio metadata for all audio streams.
        let mut audio_stream_indices: Vec<usize> = Vec::new();
        let mut all_audio_metadata: Vec<AudioMetadata> = Vec::new();

        for stream in input_context.streams() {
            if stream.parameters().medium() != Type::Audio {
                continue;
            }

            let index = stream.index();
            let track_index = audio_stream_indices.len();
            audio_stream_indices.push(index);

            let codec_parameters = stream.parameters();
            let decoder_context =
                CodecContext::from_parameters(codec_parameters).map_err(|error| {
                    UnbundleError::FileOpen {
                        path: canonical_path.clone(),
                        reason: format!(
                            "Failed to read audio codec parameters for stream {index}: {error}"
                        ),
                    }
                })?;
            let audio_decoder =
                decoder_context
                    .decoder()
                    .audio()
                    .map_err(|error| UnbundleError::FileOpen {
                        path: canonical_path.clone(),
                        reason: format!(
                            "Failed to create audio decoder for stream {index}: {error}"
                        ),
                    })?;

            let sample_rate = audio_decoder.rate();
            let channels = audio_decoder.channels();
            let bit_rate = audio_decoder.bit_rate() as u64;

            let codec_name = audio_decoder
                .codec()
                .map(|codec| codec.name().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            all_audio_metadata.push(AudioMetadata {
                sample_rate,
                channels,
                codec: codec_name,
                bit_rate,
                track_index,
                stream_index: index,
            });
        }

        // Default audio is the "best" stream as selected by FFmpeg.
        let audio_metadata = if let Some(best_index) = audio_stream_index {
            all_audio_metadata
                .iter()
                .find(|m| m.stream_index == best_index)
                .cloned()
        } else {
            all_audio_metadata.first().cloned()
        };

        let audio_tracks = if all_audio_metadata.is_empty() {
            None
        } else {
            Some(all_audio_metadata)
        };

        // Extract subtitle stream metadata.
        let subtitle_stream_index = input_context
            .streams()
            .best(Type::Subtitle)
            .map(|stream| stream.index());

        let mut subtitle_stream_indices: Vec<usize> = Vec::new();
        let mut all_subtitle_metadata: Vec<SubtitleMetadata> = Vec::new();

        for stream in input_context.streams() {
            if stream.parameters().medium() != Type::Subtitle {
                continue;
            }

            let index = stream.index();
            let track_index = subtitle_stream_indices.len();
            subtitle_stream_indices.push(index);

            let codec_parameters = stream.parameters();
            let decoder_context =
                CodecContext::from_parameters(codec_parameters).ok();

            let codec_name = decoder_context
                .and_then(|ctx| {
                    let name = ctx.id().name();
                    if name.is_empty() { None } else { Some(name.to_string()) }
                })
                .unwrap_or_else(|| "unknown".to_string());

            // Try to read language tag from stream metadata.
            let language = stream
                .metadata()
                .get("language")
                .map(|s| s.to_string());

            all_subtitle_metadata.push(SubtitleMetadata {
                codec: codec_name,
                language,
                track_index,
                stream_index: index,
            });
        }

        let subtitle_metadata = if let Some(best_index) = subtitle_stream_index {
            all_subtitle_metadata
                .iter()
                .find(|m| m.stream_index == best_index)
                .cloned()
        } else {
            all_subtitle_metadata.first().cloned()
        };

        let subtitle_tracks = if all_subtitle_metadata.is_empty() {
            None
        } else {
            Some(all_subtitle_metadata)
        };

        // Extract chapter metadata.
        let chapters = if input_context.nb_chapters() > 0 {
            let mut chapter_list = Vec::with_capacity(input_context.nb_chapters() as usize);
            for (index, chapter) in input_context.chapters().enumerate() {
                let time_base = chapter.time_base();
                let start_seconds = crate::utilities::pts_to_seconds(
                    chapter.start(),
                    time_base,
                );
                let end_seconds = crate::utilities::pts_to_seconds(
                    chapter.end(),
                    time_base,
                );
                let title = chapter.metadata().get("title").map(|s| s.to_string());

                chapter_list.push(ChapterMetadata {
                    title,
                    start: Duration::from_secs_f64(start_seconds),
                    end: Duration::from_secs_f64(end_seconds),
                    index,
                    id: chapter.id(),
                });
            }
            Some(chapter_list)
        } else {
            None
        };

        let metadata = MediaMetadata {
            video: video_metadata,
            audio: audio_metadata,
            audio_tracks,
            subtitle: subtitle_metadata,
            subtitle_tracks,
            chapters,
            duration,
            format,
        };

        Ok(Self {
            input_context,
            metadata,
            video_stream_index,
            audio_stream_index,
            audio_stream_indices,
            subtitle_stream_index,
            subtitle_stream_indices,
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
        let stream_index = self.audio_stream_index;
        AudioExtractor {
            unbundler: self,
            stream_index,
        }
    }

    /// Obtain an [`AudioExtractor`] for a specific audio track.
    ///
    /// `track_index` is the zero-based index into
    /// [`MediaMetadata::audio_tracks`].
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::NoAudioStream`] if `track_index` is out of
    /// range.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{AudioFormat, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("multi_audio.mkv")?;
    /// // Extract the second audio track
    /// let audio = unbundler.audio_track(1)?.extract(AudioFormat::Wav)?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn audio_track(&mut self, track_index: usize) -> Result<AudioExtractor<'_>, UnbundleError> {
        let stream_index = self
            .audio_stream_indices
            .get(track_index)
            .copied()
            .ok_or(UnbundleError::NoAudioStream)?;

        Ok(AudioExtractor {
            unbundler: self,
            stream_index: Some(stream_index),
        })
    }

    /// Validate the media file and return a report.
    ///
    /// Inspects cached metadata for potential issues such as missing streams,
    /// zero dimensions, or unusual frame rates. Does not re-read the file.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::MediaUnbundler;
    ///
    /// let unbundler = MediaUnbundler::open("input.mp4")?;
    /// let report = unbundler.validate();
    /// println!("{report}");
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn validate(&self) -> crate::validation::ValidationReport {
        crate::validation::validate_metadata(&self.metadata)
    }

    /// Obtain a [`SubtitleExtractor`] for the best subtitle track.
    ///
    /// The returned extractor borrows this unbundler mutably, so you cannot
    /// hold extractors for video, audio, and subtitles simultaneously.
    pub fn subtitle(&mut self) -> SubtitleExtractor<'_> {
        let stream_index = self.subtitle_stream_index;
        SubtitleExtractor {
            unbundler: self,
            stream_index,
        }
    }

    /// Obtain a [`SubtitleExtractor`] for a specific subtitle track.
    ///
    /// `track_index` is the zero-based index into
    /// [`MediaMetadata::subtitle_tracks`].
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::NoSubtitleStream`] if `track_index` is out of
    /// range.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaUnbundler, SubtitleFormat};
    ///
    /// let mut unbundler = MediaUnbundler::open("multi_sub.mkv")?;
    /// unbundler.subtitle_track(1)?.save("subs_track2.srt", SubtitleFormat::Srt)?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn subtitle_track(
        &mut self,
        track_index: usize,
    ) -> Result<SubtitleExtractor<'_>, UnbundleError> {
        let stream_index = self
            .subtitle_stream_indices
            .get(track_index)
            .copied()
            .ok_or(UnbundleError::NoSubtitleStream)?;

        Ok(SubtitleExtractor {
            unbundler: self,
            stream_index: Some(stream_index),
        })
    }
}
