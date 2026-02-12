//! Media metadata types.
//!
//! This module defines the metadata structures returned by
//! [`MediaFile::metadata`](crate::MediaFile::metadata). Metadata is
//! extracted once when the file is opened and cached for the lifetime of the
//! unbundler.

use std::collections::HashMap;
use std::time::Duration;

/// Complete metadata for a media file.
///
/// Contains optional video and audio stream metadata, plus container-level
/// information such as total duration and format name.
///
/// # Example
///
/// ```no_run
/// use unbundle::{MediaFile, UnbundleError};
///
/// let unbundler = MediaFile::open("input.mp4").unwrap();
/// let metadata = unbundler.metadata();
/// println!("Duration: {:?}", metadata.duration);
/// println!("Format: {}", metadata.format);
/// if let Some(tracks) = metadata.audio_tracks.as_ref() {
///     println!("Audio tracks: {}", tracks.len());
/// }
/// ```
#[derive(Debug, Clone)]
#[must_use]
pub struct MediaMetadata {
    /// Video stream metadata, if a video stream is present.
    pub video: Option<VideoMetadata>,
    /// Metadata for all video tracks in the file.
    ///
    /// `None` if there are no video streams. When present, the first entry
    /// matches [`video`](MediaMetadata::video) (the "best" stream).
    pub video_tracks: Option<Vec<VideoMetadata>>,
    /// Audio stream metadata for the best (default) audio stream.
    pub audio: Option<AudioMetadata>,
    /// Metadata for all audio tracks in the file.
    ///
    /// `None` if there are no audio streams. When present, the first entry
    /// matches [`audio`](MediaMetadata::audio) (the "best" stream).
    pub audio_tracks: Option<Vec<AudioMetadata>>,
    /// Subtitle stream metadata for the best subtitle stream.
    pub subtitle: Option<SubtitleMetadata>,
    /// Metadata for all subtitle tracks in the file.
    pub subtitle_tracks: Option<Vec<SubtitleMetadata>>,
    /// Chapter metadata, if the container contains chapters.
    ///
    /// Chapters represent named time segments (e.g. scenes, acts) embedded in
    /// the container. `None` when no chapters are present.
    pub chapters: Option<Vec<ChapterMetadata>>,
    /// Total duration of the media file.
    pub duration: Duration,
    /// Container format name (e.g. `"mp4"`, `"matroska"`, `"avi"`).
    pub format: String,
    /// Container-level metadata tags (e.g. title, artist, album, date).
    ///
    /// `None` when the container has no metadata tags.
    pub tags: Option<HashMap<String, String>>,
}

/// Metadata for a video stream.
///
/// Includes dimensions, frame rate, estimated frame count, codec name,
/// and colorspace information.
#[derive(Debug, Clone)]
#[must_use]
pub struct VideoMetadata {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frames per second (may be approximate for variable-frame-rate content).
    pub frames_per_second: f64,
    /// Estimated total number of frames, computed from duration and frame rate.
    pub frame_count: u64,
    /// Codec name (e.g. `"h264"`, `"vp9"`, `"av1"`).
    pub codec: String,
    /// Color space (e.g. `"BT709"`, `"BT2020NCL"`), if available.
    pub color_space: Option<String>,
    /// Color range (`"TV"` for limited, `"PC"` for full), if available.
    pub color_range: Option<String>,
    /// Color primaries (e.g. `"BT709"`, `"BT2020"`), if available.
    pub color_primaries: Option<String>,
    /// Color transfer characteristics (e.g. `"SMPTE2084"` for HDR10 PQ), if available.
    pub color_transfer: Option<String>,
    /// Bits per raw sample (e.g. 8, 10, 12), if available.
    pub bits_per_raw_sample: Option<u32>,
    /// Pixel format name (e.g. `"yuv420p"`, `"yuv420p10le"`), if available.
    pub pixel_format_name: Option<String>,
    /// Zero-based track number among all video streams in the file.
    pub track_index: usize,
    /// FFmpeg stream index within the container.
    pub(crate) stream_index: usize,
}

/// Metadata for an audio stream.
///
/// Includes sample rate, channel count, codec name, and bit rate.
/// When multiple audio tracks exist, use
/// [`track_index`](AudioMetadata::track_index) to identify each.
#[derive(Debug, Clone)]
#[must_use]
pub struct AudioMetadata {
    /// Sample rate in hertz (e.g. `44100`, `48000`).
    pub sample_rate: u32,
    /// Number of audio channels (e.g. `2` for stereo).
    pub channels: u16,
    /// Codec name (e.g. `"aac"`, `"mp3"`, `"flac"`).
    pub codec: String,
    /// Bit rate in bits per second.
    pub bit_rate: u64,
    /// Zero-based track number among all audio streams in the file.
    pub track_index: usize,
    /// FFmpeg stream index within the container.
    pub(crate) stream_index: usize,
}

/// Metadata for a chapter within a media file.
///
/// Chapters represent named time segments (e.g. scenes, acts, or songs)
/// embedded in the container by the authoring tool. Not all containers
/// support chapters; when present they are extracted at open time and
/// stored in [`MediaMetadata::chapters`].
///
/// # Example
///
/// ```no_run
/// use unbundle::{MediaFile, UnbundleError};
///
/// let unbundler = MediaFile::open("input.mkv")?;
/// if let Some(chapters) = unbundler.metadata().chapters.as_ref() {
///     for chapter in chapters {
///         println!("[{:?}–{:?}] {}", chapter.start, chapter.end,
///             chapter.title.as_deref().unwrap_or("(untitled)"));
///     }
/// }
/// # Ok::<(), UnbundleError>(())
/// ```
#[derive(Debug, Clone)]
#[must_use]
pub struct ChapterMetadata {
    /// Human-readable chapter title, if tagged (e.g. `"Opening Credits"`).
    pub title: Option<String>,
    /// Start time of the chapter.
    pub start: Duration,
    /// End time of the chapter.
    pub end: Duration,
    /// Zero-based chapter index within the container.
    pub index: usize,
    /// The chapter's unique identifier as stored in the container.
    pub id: i64,
}

/// Metadata for a subtitle stream.
///
/// Includes codec name, language (if tagged), and track index.
#[derive(Debug, Clone)]
#[must_use]
pub struct SubtitleMetadata {
    /// Codec name (e.g. `"subrip"`, `"ass"`, `"mov_text"`).
    pub codec: String,
    /// Language tag from stream metadata (e.g. `"eng"`, `"fre"`), if available.
    pub language: Option<String>,
    /// Zero-based track number among all subtitle streams in the file.
    pub track_index: usize,
    /// FFmpeg stream index within the container.
    pub(crate) stream_index: usize,
}
