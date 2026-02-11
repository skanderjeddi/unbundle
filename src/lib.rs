//! # unbundle
//!
//! Unbundle media files — extract still frames and audio from video files.
//!
//! `unbundle` provides a clean, ergonomic API for extracting video frames as
//! [`image::DynamicImage`] values and audio tracks as encoded byte vectors,
//! powered by FFmpeg via the [`ffmpeg-next`](https://crates.io/crates/ffmpeg-next)
//! crate.
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
//! ## Features
//!
//! - **Frame extraction** by frame number, timestamp, range, interval, or
//!   specific frame list
//! - **Audio extraction** to WAV, MP3, FLAC, or AAC
//! - **In-memory** or **file-based** audio output
//! - **Rich metadata** for both video and audio streams
//! - **Efficient seeking** — seeks to nearest keyframe, then decodes forward
//!
//! ## Requirements
//!
//! FFmpeg development libraries must be installed on your system. See the
//! [README](https://github.com/example/unbundle#installation) for
//! platform-specific instructions.

pub mod audio;
#[cfg(feature = "async-tokio")]
pub mod stream;
pub mod config;
pub mod convert;
pub mod error;
pub mod iterator;
#[cfg(feature = "hw-accel")]
pub mod hwaccel;
pub mod metadata;
#[cfg(feature = "parallel")]
mod parallel;
pub mod progress;
#[cfg(feature = "scene-detection")]
pub mod scene;
pub mod subtitle;
pub mod unbundler;
mod utilities;
pub mod validation;
pub mod video;

#[cfg(feature = "async-tokio")]
pub use stream::{AudioFuture, FrameStream};
pub use audio::{AudioExtractor, AudioFormat};
pub use config::{ExtractionConfig, FrameOutputConfig, OutputPixelFormat};
pub use convert::Remuxer;
pub use error::UnbundleError;
pub use iterator::FrameIterator;
#[cfg(feature = "hw-accel")]
pub use hwaccel::{HwAccelMode, HwDeviceType};
pub use metadata::{AudioMetadata, MediaMetadata, SubtitleMetadata, VideoMetadata};
pub use progress::{CancellationToken, OperationType, ProgressCallback, ProgressInfo};
#[cfg(feature = "scene-detection")]
pub use scene::{SceneChange, SceneDetectionConfig};
pub use subtitle::{SubtitleEntry, SubtitleExtractor, SubtitleFormat};
pub use unbundler::MediaUnbundler;
pub use validation::ValidationReport;
pub use video::{FrameRange, VideoExtractor};
