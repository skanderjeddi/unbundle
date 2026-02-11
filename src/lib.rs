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
//! use unbundle::MediaUnbundler;
//!
//! let mut unbundler = MediaUnbundler::open("input.mp4").unwrap();
//! let frame = unbundler.video().frame(0).unwrap();
//! frame.save("first_frame.png").unwrap();
//! ```
//!
//! ### Extract Audio
//!
//! ```no_run
//! use unbundle::{AudioFormat, MediaUnbundler};
//!
//! let mut unbundler = MediaUnbundler::open("input.mp4").unwrap();
//! unbundler.audio().save("output.wav", AudioFormat::Wav).unwrap();
//! ```
//!
//! ### Extract Frames by Range
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use unbundle::{FrameRange, MediaUnbundler};
//!
//! let mut unbundler = MediaUnbundler::open("input.mp4").unwrap();
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
//! use unbundle::{MediaUnbundler, SubtitleFormat};
//!
//! let mut unbundler = MediaUnbundler::open("input.mkv").unwrap();
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
//! - **Rich metadata** — video dimensions, frame rate, frame count, audio
//!   sample rate, channels, codec info, multi-track audio/subtitle metadata
//! - **Configurable output** — pixel format (RGB8, RGBA8, GRAY8) and target
//!   resolution with aspect ratio preservation
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
//! | `async-tokio` | `FrameStream` and `AudioFuture` for async extraction via Tokio |
//! | `parallel` | `frames_parallel()` distributes decoding across rayon threads |
//! | `hw-accel` | Hardware-accelerated decoding (CUDA, VAAPI, DXVA2, D3D11VA, VideoToolbox, QSV) |
//! | `scene-detection` | Scene change detection via FFmpeg's `scdet` filter |
//! | `full` | Enables all of the above |
//!
//! ## Requirements
//!
//! FFmpeg development libraries must be installed on your system. See the
//! [README](https://github.com/skanderjeddi/unbundle#installation) for
//! platform-specific instructions.

pub mod audio;
#[cfg(feature = "async-tokio")]
pub mod stream;
pub mod config;
pub mod remux;
pub mod error;
pub mod iterator;
#[cfg(feature = "hw-accel")]
pub mod hw_accel;
pub mod metadata;
#[cfg(feature = "parallel")]
mod parallel;
pub mod probe;
pub mod progress;
#[cfg(feature = "scene-detection")]
pub mod scene;
pub mod subtitle;
pub mod thumbnail;
pub mod unbundler;
mod utilities;
pub mod validation;
pub mod video;

#[cfg(feature = "async-tokio")]
pub use stream::{AudioFuture, FrameStream};
pub use audio::{AudioExtractor, AudioFormat};
pub use config::{ExtractionConfig, FrameOutputConfig, PixelFormat};
pub use remux::Remuxer;
pub use error::UnbundleError;
pub use iterator::FrameIterator;
#[cfg(feature = "hw-accel")]
pub use hw_accel::{HwAccelMode, HwDeviceType};
pub use metadata::{AudioMetadata, ChapterMetadata, MediaMetadata, SubtitleMetadata, VideoMetadata};
pub use probe::MediaProbe;
pub use progress::{CancellationToken, OperationType, ProgressCallback, ProgressInfo};
#[cfg(feature = "scene-detection")]
pub use scene::{SceneChange, SceneDetectionConfig};
pub use subtitle::{SubtitleEvent, SubtitleExtractor, SubtitleFormat};
pub use thumbnail::{ThumbnailConfig, ThumbnailGenerator};
pub use unbundler::MediaUnbundler;
pub use validation::ValidationReport;
pub use video::{FrameInfo, FrameRange, FrameType, VideoExtractor};
