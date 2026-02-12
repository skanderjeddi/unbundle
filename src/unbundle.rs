//! Core [`MediaFile`] implementation.
//!
//! `MediaFile` is the main entry point for the crate. It opens a media
//! file, extracts and caches metadata, and provides access to
//! [`VideoHandle`] and
//! [`AudioHandle`] for frame and audio
//! extraction respectively.

use std::{
    collections::HashMap,
    fmt::{Debug, Formatter, Result as FmtResult},
    path::{Path, PathBuf},
    time::Duration,
};

use ffmpeg_next::{codec::context::Context as CodecContext, format::context::Input, media::Type};

use crate::{
    audio::AudioHandle,
    error::UnbundleError,
    metadata::{AudioMetadata, ChapterMetadata, MediaMetadata, SubtitleMetadata, VideoMetadata},
    packet_iterator::PacketIterator,
    subtitle::SubtitleHandle,
    video::VideoHandle,
};

/// Main struct for unbundling media files.
///
/// Created via [`MediaFile::open`], this struct holds the demuxer context
/// and cached metadata. Use [`video()`](MediaFile::video) and
/// [`audio()`](MediaFile::audio) to obtain extractors for frames and
/// audio respectively.
///
/// # Example
///
/// ```no_run
/// use unbundle::{MediaFile, UnbundleError};
///
/// let mut unbundler = MediaFile::open("input.mp4").unwrap();
/// let metadata = unbundler.metadata();
/// println!("Duration: {:?}", metadata.duration);
///
/// // Extract a single frame
/// let frame = unbundler.video().frame(0).unwrap();
/// frame.save("first_frame.png").unwrap();
/// ```
pub struct MediaFile {
    /// The opened FFmpeg input (demuxer) context.
    pub(crate) input_context: Input,
    /// Cached metadata extracted at open time.
    pub(crate) metadata: MediaMetadata,
    /// Index of the best video stream, if one exists.
    pub(crate) video_stream_index: Option<usize>,
    /// Indices of all video streams, ordered by track number.
    pub(crate) video_stream_indices: Vec<usize>,
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

impl Debug for MediaFile {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("MediaFile")
            .field("metadata", &self.metadata)
            .field("video_stream_index", &self.video_stream_index)
            .field("video_stream_indices", &self.video_stream_indices)
            .field("audio_stream_index", &self.audio_stream_index)
            .field("audio_stream_indices", &self.audio_stream_indices)
            .field("subtitle_stream_index", &self.subtitle_stream_index)
            .field("subtitle_stream_indices", &self.subtitle_stream_indices)
            .field("file_path", &self.file_path)
            .finish_non_exhaustive()
    }
}

impl MediaFile {
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
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let unbundler = MediaFile::open("video.mp4")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, UnbundleError> {
        let path = path.as_ref();
        let canonical_path = path.to_path_buf();

        log::debug!("Opening media file: {}", canonical_path.display());

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

        // Extract container-level metadata tags.
        let tags = {
            let mut map = HashMap::new();
            for (key, value) in input_context.metadata().iter() {
                map.insert(key.to_string(), value.to_string());
            }
            if map.is_empty() { None } else { Some(map) }
        };

        // Extract video metadata for all video streams.
        let mut video_stream_indices: Vec<usize> = Vec::new();
        let mut all_video_metadata: Vec<VideoMetadata> = Vec::new();

        for stream in input_context.streams() {
            if stream.parameters().medium() != Type::Video {
                continue;
            }

            let index = stream.index();
            let track_index = video_stream_indices.len();
            video_stream_indices.push(index);

            let codec_parameters = stream.parameters();
            let decoder_context =
                CodecContext::from_parameters(codec_parameters).map_err(|error| {
                    UnbundleError::FileOpen {
                        path: canonical_path.clone(),
                        reason: format!(
                            "Failed to read video codec parameters for stream {index}: {error}"
                        ),
                    }
                })?;
            let video_decoder =
                decoder_context
                    .decoder()
                    .video()
                    .map_err(|error| UnbundleError::FileOpen {
                        path: canonical_path.clone(),
                        reason: format!(
                            "Failed to create video decoder for stream {index}: {error}"
                        ),
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

            // Extract colorspace / HDR metadata.
            let color_space = {
                let cs = video_decoder.color_space();
                let s = format!("{cs:?}");
                if s == "Unspecified" { None } else { Some(s) }
            };
            let color_range = {
                let cr = video_decoder.color_range();
                let s = format!("{cr:?}");
                if s == "Unspecified" { None } else { Some(s) }
            };
            let color_primaries = {
                let cp = video_decoder.color_primaries();
                let s = format!("{cp:?}");
                if s == "Unspecified" { None } else { Some(s) }
            };
            let color_transfer = {
                let ct = video_decoder.color_transfer_characteristic();
                let s = format!("{ct:?}");
                if s == "Unspecified" { None } else { Some(s) }
            };
            let bits_per_raw_sample = {
                let par = stream.parameters();
                let raw_par = unsafe { *par.as_ptr() };
                let bits = raw_par.bits_per_raw_sample;
                if bits > 0 { Some(bits as u32) } else { None }
            };
            let pixel_format_name = {
                let pf = video_decoder.format();
                let name = format!("{pf:?}");
                if name == "None" { None } else { Some(name) }
            };

            all_video_metadata.push(VideoMetadata {
                width,
                height,
                frames_per_second,
                frame_count,
                codec: codec_name,
                color_space,
                color_range,
                color_primaries,
                color_transfer,
                bits_per_raw_sample,
                pixel_format_name,
                track_index,
                stream_index: index,
            });
        }

        // Default video is the "best" stream as selected by FFmpeg.
        let video_metadata = if let Some(best_index) = video_stream_index {
            all_video_metadata
                .iter()
                .find(|m| m.stream_index == best_index)
                .cloned()
        } else {
            all_video_metadata.first().cloned()
        };

        let video_tracks = if all_video_metadata.is_empty() {
            None
        } else {
            Some(all_video_metadata)
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
            let decoder_context = CodecContext::from_parameters(codec_parameters).ok();

            let codec_name = decoder_context
                .and_then(|ctx| {
                    let name = ctx.id().name();
                    if name.is_empty() {
                        None
                    } else {
                        Some(name.to_string())
                    }
                })
                .unwrap_or_else(|| "unknown".to_string());

            // Try to read language tag from stream metadata.
            let language = stream.metadata().get("language").map(|s| s.to_string());

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
                let start_seconds = crate::conversion::pts_to_seconds(chapter.start(), time_base);
                let end_seconds = crate::conversion::pts_to_seconds(chapter.end(), time_base);
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
            video_tracks,
            audio: audio_metadata,
            audio_tracks,
            subtitle: subtitle_metadata,
            subtitle_tracks,
            chapters,
            duration,
            format,
            tags,
        };

        log::info!(
            "Opened media file: {} (format={}, duration={:.2}s, video_streams={}, audio_streams={}, subtitle_streams={})",
            canonical_path.display(),
            metadata.format,
            metadata.duration.as_secs_f64(),
            video_stream_indices.len(),
            audio_stream_indices.len(),
            subtitle_stream_indices.len(),
        );

        if let Some(video) = &metadata.video {
            log::debug!(
                "Best video stream: index={}, {}x{}, {:.2} fps, codec={}, ~{} frames",
                video.stream_index,
                video.width,
                video.height,
                video.frames_per_second,
                video.codec,
                video.frame_count,
            );
        }

        if let Some(audio) = &metadata.audio {
            log::debug!(
                "Best audio stream: index={}, {} Hz, {} ch, codec={}",
                audio.stream_index,
                audio.sample_rate,
                audio.channels,
                audio.codec,
            );
        }

        Ok(Self {
            input_context,
            metadata,
            video_stream_index,
            video_stream_indices,
            audio_stream_index,
            audio_stream_indices,
            subtitle_stream_index,
            subtitle_stream_indices,
            file_path: canonical_path,
        })
    }

    /// Get a reference to the cached media metadata.
    ///
    /// Metadata is extracted once during [`open`](MediaFile::open) and
    /// does not require additional decoding.
    pub fn metadata(&self) -> &MediaMetadata {
        &self.metadata
    }

    /// Create a lazy iterator over all demuxed packets.
    ///
    /// The iterator yields [`PacketInfo`](crate::PacketInfo) structs
    /// containing stream index, PTS, DTS, size and keyframe flag for
    /// each packet without decoding.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// for pkt in unbundler.packet_iter()? {
    ///     let pkt = pkt?;
    ///     println!("stream={} pts={:?} key={}", pkt.stream_index, pkt.pts, pkt.is_keyframe);
    /// }
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn packet_iter(&mut self) -> Result<PacketIterator<'_>, UnbundleError> {
        Ok(PacketIterator::new(self))
    }

    /// Obtain a [`VideoHandle`] for extracting video frames.
    ///
    /// The returned extractor borrows this unbundler mutably, so you cannot
    /// hold extractors for both video and audio simultaneously.
    pub fn video(&mut self) -> VideoHandle<'_> {
        VideoHandle {
            unbundler: self,
            stream_index: None,
        }
    }

    /// Obtain a [`VideoHandle`] for a specific video track.
    ///
    /// `track_index` is the zero-based index into
    /// [`MediaMetadata::video_tracks`].
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::VideoTrackOutOfRange`] if `track_index` is out of
    /// range.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("multi_video.mkv")?;
    /// // Extract a frame from the second video track
    /// let frame = unbundler.video_track(1)?.frame(0)?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn video_track(&mut self, track_index: usize) -> Result<VideoHandle<'_>, UnbundleError> {
        let stream_index = self.video_stream_indices.get(track_index).copied().ok_or(
            UnbundleError::VideoTrackOutOfRange {
                track_index,
                track_count: self.video_stream_indices.len(),
            },
        )?;

        Ok(VideoHandle {
            unbundler: self,
            stream_index: Some(stream_index),
        })
    }

    /// Obtain an [`AudioHandle`] for extracting audio data.
    ///
    /// The returned extractor borrows this unbundler mutably, so you cannot
    /// hold extractors for both video and audio simultaneously.
    pub fn audio(&mut self) -> AudioHandle<'_> {
        let stream_index = self.audio_stream_index;
        AudioHandle {
            unbundler: self,
            stream_index,
        }
    }

    /// Obtain an [`AudioHandle`] for a specific audio track.
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
    /// use unbundle::{AudioFormat, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("multi_audio.mkv")?;
    /// // Extract the second audio track
    /// let audio = unbundler.audio_track(1)?.extract(AudioFormat::Wav)?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn audio_track(&mut self, track_index: usize) -> Result<AudioHandle<'_>, UnbundleError> {
        let stream_index = self
            .audio_stream_indices
            .get(track_index)
            .copied()
            .ok_or(UnbundleError::NoAudioStream)?;

        Ok(AudioHandle {
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
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let unbundler = MediaFile::open("input.mp4")?;
    /// let report = unbundler.validate();
    /// println!("{report}");
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn validate(&self) -> crate::validation::ValidationReport {
        crate::validation::validate_metadata(&self.metadata)
    }

    /// Obtain a [`SubtitleHandle`] for the best subtitle track.
    ///
    /// The returned extractor borrows this unbundler mutably, so you cannot
    /// hold extractors for video, audio, and subtitles simultaneously.
    pub fn subtitle(&mut self) -> SubtitleHandle<'_> {
        let stream_index = self.subtitle_stream_index;
        SubtitleHandle {
            unbundler: self,
            stream_index,
        }
    }

    /// Obtain a [`SubtitleHandle`] for a specific subtitle track.
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
    /// use unbundle::{MediaFile, SubtitleFormat, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("multi_sub.mkv")?;
    /// unbundler.subtitle_track(1)?.save("subs_track2.srt", SubtitleFormat::Srt)?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn subtitle_track(
        &mut self,
        track_index: usize,
    ) -> Result<SubtitleHandle<'_>, UnbundleError> {
        let stream_index = self
            .subtitle_stream_indices
            .get(track_index)
            .copied()
            .ok_or(UnbundleError::NoSubtitleStream)?;

        Ok(SubtitleHandle {
            unbundler: self,
            stream_index: Some(stream_index),
        })
    }
}
