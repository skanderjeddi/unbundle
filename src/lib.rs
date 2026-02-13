//! # unbundle
//!
//! Unbundle media files — extract still frames, audio tracks, and subtitles
//! from video files.
//!
//! `unbundle` provides a clean, ergonomic API for extracting video frames as
//! [`image::DynamicImage`] values, audio tracks as encoded byte vectors, and
//! subtitle tracks as structured text, powered by FFmpeg via the
//! [`ffmpeg-next`](https://crates.io/crates/ffmpeg-next) crate.
//!
//! ## Quick Start
//!
//! ### Extract a Video Frame
//!
//! ```no_run
//! use unbundle::MediaFile;
//!
//! let mut unbundler = MediaFile::open("input.mp4").unwrap();
//! let frame = unbundler.video().frame(0).unwrap();
//! frame.save("first_frame.png").unwrap();
//! ```
//!
//! ### Extract Audio
//!
//! ```no_run
//! use unbundle::{AudioFormat, MediaFile};
//!
//! let mut unbundler = MediaFile::open("input.mp4").unwrap();
//! unbundler.audio().save("output.wav", AudioFormat::Wav).unwrap();
//! ```
//!
//! ### Extract Frames by Range
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use unbundle::{FrameRange, MediaFile};
//!
//! let mut unbundler = MediaFile::open("input.mp4").unwrap();
//!
//! // Every 30th frame
//! let frames = unbundler.video().frames(FrameRange::Interval(30)).unwrap();
//!
//! // Frames between two timestamps
//! let frames = unbundler.video().frames(
//!     FrameRange::TimeRange(Duration::from_secs(10), Duration::from_secs(20))
//! ).unwrap();
//! ```
//!
//! ### Extract Subtitles
//!
//! ```no_run
//! use unbundle::{MediaFile, SubtitleFormat};
//!
//! let mut unbundler = MediaFile::open("input.mkv").unwrap();
//! unbundler.subtitle().save("output.srt", SubtitleFormat::Srt).unwrap();
//! ```
//!
//! ## Features
//!
//! - **Frame extraction** — by frame number, timestamp, range, interval, or
//!   specific frame list
//! - **Audio extraction** — to WAV, MP3, FLAC, or AAC (file or in-memory)
//! - **Subtitle extraction** — decode text-based subtitles to SRT, WebVTT, or
//!   raw text
//! - **Container remuxing** — lossless format conversion (e.g. MKV → MP4)
//! - **Raw stream copy** — packet-level stream extraction to file/memory without re-encoding
//! - **Rich metadata** — video dimensions, frame rate, frame count, audio
//!   sample rate, channels, codec info, multi-track audio/subtitle metadata
//! - **Configurable output** — pixel format (RGB8, RGBA8, GRAY8) and target
//!   resolution with aspect ratio preservation
//! - **Custom FFmpeg filters** — apply filter graphs during frame extraction
//! - **Progress & cancellation** — cooperative callbacks and
//!   `CancellationToken` for long-running operations
//! - **Streaming iteration** — lazy `FrameIterator` (pull-based) and
//!   `for_each_frame` (push-based) without buffering entire frame sets
//! - **Validation** — inspect media files for structural issues before
//!   extraction
//! - **Chapter support** — extract chapter metadata (titles, timestamps)
//! - **Frame metadata** — per-frame decode info (PTS, keyframe, picture type)
//! - **Segmented extraction** — extract from multiple disjoint time ranges
//! - **Stream probing** — lightweight `MediaProbe` for quick inspection
//! - **Thumbnail helpers** — single thumbnails, grids, and smart selection
//! - **Efficient seeking** — seeks to nearest keyframe, then decodes forward
//! - **Zero-copy in-memory audio** — uses FFmpeg's dynamic buffer I/O
//!
//! ### Optional Features
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `async` | `FrameStream` and `AudioFuture` for async extraction via Tokio |
//! | `rayon` | `frames_parallel()` distributes decoding across rayon threads |
//! | `hardware` | Hardware-accelerated decoding (CUDA, VAAPI, DXVA2, D3D11VA, VideoToolbox, QSV) |
//! | `scene` | Scene change detection via FFmpeg's `scdet` filter |
//! | `full` | Enables all of the above |
//!
//! ## Requirements
//!
//! FFmpeg development libraries must be installed on your system. See the
//! [README](https://github.com/skanderjeddi/unbundle#installation) for
//! platform-specific instructions.

pub mod audio;
pub mod audio_iterator;
pub mod configuration;
mod conversion;
#[cfg(feature = "encode")]
pub mod encode;
pub mod error;
pub mod ffmpeg;
#[cfg(feature = "gif")]
pub mod gif;
#[cfg(feature = "hardware")]
pub mod hardware_acceleration;
pub mod keyframe;
#[cfg(feature = "loudness")]
pub mod loudness;
pub mod metadata;
pub mod packet_iterator;
pub mod probe;
pub mod progress;
#[cfg(feature = "rayon")]
mod rayon;
pub mod remux;
#[cfg(feature = "scene")]
pub mod scene;
#[cfg(feature = "async")]
pub mod stream;
pub mod subtitle;
pub mod thumbnail;
#[cfg(feature = "transcode")]
pub mod transcode;
pub mod unbundle;
pub mod validation;
pub mod variable_framerate;
pub mod video;
pub mod video_iterator;
#[cfg(feature = "waveform")]
pub mod waveform;

pub use audio::{AudioFormat, AudioHandle};
pub use audio_iterator::{AudioChunk, AudioIterator};
pub use configuration::{ExtractOptions, FrameOutputOptions, PixelFormat};
#[cfg(feature = "encode")]
pub use encode::{VideoCodec, VideoEncoder, VideoEncoderOptions};
pub use error::UnbundleError;
pub use ffmpeg::{FfmpegLogLevel, get_ffmpeg_log_level, set_ffmpeg_log_level};
#[cfg(feature = "gif")]
pub use gif::GifOptions;
#[cfg(feature = "hardware")]
pub use hardware_acceleration::{HardwareAccelerationMode, HardwareDeviceType};
pub use keyframe::{GroupOfPicturesInfo, KeyFrameMetadata};
#[cfg(feature = "loudness")]
pub use loudness::LoudnessInfo;
pub use metadata::{
    AudioMetadata, ChapterMetadata, MediaMetadata, SubtitleMetadata, VideoMetadata,
};
pub use packet_iterator::{PacketInfo, PacketIterator};
pub use probe::MediaProbe;
pub use progress::{CancellationToken, OperationType, ProgressCallback, ProgressInfo};
pub use remux::Remuxer;
#[cfg(feature = "scene")]
pub use scene::{SceneChange, SceneDetectionMode, SceneDetectionOptions};
#[cfg(feature = "async")]
pub use stream::{AudioFuture, FrameStream};
pub use subtitle::{BitmapSubtitleEvent, SubtitleEvent, SubtitleFormat, SubtitleHandle};
pub use thumbnail::{ThumbnailHandle, ThumbnailOptions};
#[cfg(feature = "transcode")]
pub use transcode::Transcoder;
pub use unbundle::MediaFile;
pub use validation::ValidationReport;
pub use variable_framerate::VariableFrameRateAnalysis;
pub use video::{FrameMetadata, FrameRange, FrameType, VideoHandle};
pub use video_iterator::FrameIterator;
#[cfg(feature = "waveform")]
pub use waveform::{WaveformBin, WaveformData, WaveformOptions};
