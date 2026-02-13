//! FFmpeg log level configuration.
//!
//! FFmpeg has its own internal logging system, separate from the Rust
//! [`log`](https://crates.io/crates/log) crate. By default, FFmpeg prints
//! warnings and errors to stderr, which can be noisy in library usage. This
//! module provides a thin wrapper around FFmpeg's log-level API so users of
//! `unbundle` can silence or tune FFmpeg output without importing
//! `ffmpeg-next` directly.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{FfmpegLogLevel, MediaFile};
//!
//! // Silence all FFmpeg output except fatal errors.
//! unbundle::set_ffmpeg_log_level(FfmpegLogLevel::Fatal);
//!
//! // Or silence completely.
//! unbundle::set_ffmpeg_log_level(FfmpegLogLevel::Quiet);
//!
//! let mut unbundler = MediaFile::open("input.mp4").unwrap();
//! ```
//!
//! # Note
//!
//! This controls **FFmpeg's own console output**, not the Rust-side
//! diagnostic messages emitted via the `log` crate. To configure those,
//! use a standard `log` subscriber such as `env_logger` or `tracing`.

use ffmpeg_next::util::log::Level;

/// FFmpeg internal log verbosity level.
///
/// Maps directly to FFmpeg's `AV_LOG_*` constants. Setting a level causes
/// FFmpeg to suppress all messages below that severity.
///
/// # Ordering (most verbose → most quiet)
///
/// `Trace` > `Debug` > `Verbose` > `Info` > `Warning` > `Error` > `Fatal` > `Panic` > `Quiet`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FfmpegLogLevel {
    /// Print no output at all.
    Quiet,
    /// Only log when a condition that cannot be recovered from is encountered
    /// and the process will abort.
    Panic,
    /// Only log when an unrecoverable error is encountered (the context
    /// becomes invalid but the process may continue).
    Fatal,
    /// Log recoverable errors.
    Error,
    /// Log warnings (default FFmpeg level).
    Warning,
    /// Log informational messages.
    Info,
    /// Log verbose informational messages.
    Verbose,
    /// Log debugging messages.
    Debug,
    /// Extremely verbose tracing output.
    Trace,
}

impl FfmpegLogLevel {
    /// Convert to the `ffmpeg_next::util::log::Level` enum.
    fn to_ffmpeg_level(self) -> Level {
        match self {
            FfmpegLogLevel::Quiet => Level::Quiet,
            FfmpegLogLevel::Panic => Level::Panic,
            FfmpegLogLevel::Fatal => Level::Fatal,
            FfmpegLogLevel::Error => Level::Error,
            FfmpegLogLevel::Warning => Level::Warning,
            FfmpegLogLevel::Info => Level::Info,
            FfmpegLogLevel::Verbose => Level::Verbose,
            FfmpegLogLevel::Debug => Level::Debug,
            FfmpegLogLevel::Trace => Level::Trace,
        }
    }

    /// Convert from the `ffmpeg_next::util::log::Level` enum.
    fn from_ffmpeg_level(level: Level) -> Self {
        match level {
            Level::Quiet => FfmpegLogLevel::Quiet,
            Level::Panic => FfmpegLogLevel::Panic,
            Level::Fatal => FfmpegLogLevel::Fatal,
            Level::Error => FfmpegLogLevel::Error,
            Level::Warning => FfmpegLogLevel::Warning,
            Level::Info => FfmpegLogLevel::Info,
            Level::Verbose => FfmpegLogLevel::Verbose,
            Level::Debug => FfmpegLogLevel::Debug,
            Level::Trace => FfmpegLogLevel::Trace,
        }
    }
}

/// Set the FFmpeg internal log verbosity level.
///
/// This controls what FFmpeg prints to stderr. It does **not** affect
/// Rust-side `log` crate output.
///
/// # Example
///
/// ```no_run
/// use unbundle::FfmpegLogLevel;
///
/// // Only show errors and above.
/// unbundle::set_ffmpeg_log_level(FfmpegLogLevel::Error);
/// ```
pub fn set_ffmpeg_log_level(level: FfmpegLogLevel) {
    ffmpeg_next::util::log::set_level(level.to_ffmpeg_level());
}

/// Get the current FFmpeg internal log verbosity level.
///
/// Returns `None` if the current level does not map to a known variant
/// (should not happen in practice).
///
/// # Example
///
/// ```no_run
/// use unbundle::FfmpegLogLevel;
///
/// let level = unbundle::get_ffmpeg_log_level();
/// println!("Current FFmpeg log level: {:?}", level);
/// ```
pub fn get_ffmpeg_log_level() -> Option<FfmpegLogLevel> {
    ffmpeg_next::util::log::get_level()
        .ok()
        .map(FfmpegLogLevel::from_ffmpeg_level)
}
