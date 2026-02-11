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
pub mod error;
pub mod metadata;
pub mod unbundler;
mod utilities;
pub mod video;

pub use audio::{AudioExtractor, AudioFormat};
pub use error::UnbundleError;
pub use metadata::{AudioMetadata, MediaMetadata, VideoMetadata};
pub use unbundler::MediaUnbundler;
pub use video::{FrameRange, VideoExtractor};
