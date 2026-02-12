//! Raw packet-level iteration.
//!
//! This module provides [`PacketIterator`] for iterating over the
//! demuxed packets of a media file without decoding.  Each yielded
//! [`PacketInfo`] carries the stream index, PTS, DTS, size and keyframe
//! flag of a single packet.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{MediaFile, UnbundleError};
//!
//! let mut unbundler = MediaFile::open("input.mp4")?;
//! let iter = unbundler.packet_iter()?;
//! for info in iter {
//!     let pkt = info?;
//!     if pkt.is_keyframe {
//!         println!("Keyframe at PTS {:?} in stream {}", pkt.pts, pkt.stream_index);
//!     }
//! }
//! # Ok::<(), UnbundleError>(())
//! ```

use std::time::Duration;

use ffmpeg_next::{Error as FfmpegError, Packet, Rational};

use crate::error::UnbundleError;
use crate::unbundle::MediaFile;

/// Metadata for a single demuxed packet.
#[derive(Debug, Clone)]
pub struct PacketInfo {
    /// The stream index this packet belongs to.
    pub stream_index: usize,
    /// Presentation timestamp (if available).
    pub pts: Option<i64>,
    /// Decoding timestamp (if available).
    pub dts: Option<i64>,
    /// Presentation timestamp converted to [`Duration`] using the stream's
    /// time base. `None` if no PTS is present.
    pub pts_duration: Option<Duration>,
    /// Packet payload size in bytes.
    pub size: usize,
    /// Whether this packet is a keyframe / sync point.
    pub is_keyframe: bool,
    /// The stream's time base numerator / denominator.
    pub time_base: Rational,
}

/// A lazy iterator over demuxed packets.
///
/// Packets are read one at a time without decoding.  The iterator
/// borrows the underlying [`MediaFile`] mutably.
pub struct PacketIterator<'a> {
    unbundler: &'a mut MediaFile,
    /// Per-stream time bases, indexed by stream index.
    time_bases: Vec<Rational>,
    done: bool,
}

impl<'a> PacketIterator<'a> {
    /// Create a new packet iterator over all streams.
    pub(crate) fn new(unbundler: &'a mut MediaFile) -> Self {
        log::debug!("Creating PacketIterator");
        let time_bases: Vec<Rational> = unbundler
            .input_context
            .streams()
            .map(|s| s.time_base())
            .collect();

        Self {
            unbundler,
            time_bases,
            done: false,
        }
    }
}

impl<'a> Iterator for PacketIterator<'a> {
    type Item = Result<PacketInfo, UnbundleError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        let mut packet = Packet::empty();
        match packet.read(&mut self.unbundler.input_context) {
            Ok(()) => {
                let stream_index = packet.stream() as usize;
                let time_base = self
                    .time_bases
                    .get(stream_index)
                    .copied()
                    .unwrap_or(Rational::new(1, 90_000));

                let pts = packet.pts();
                let dts = packet.dts();
                let pts_duration = pts.map(|p| {
                    let seconds = p as f64
                        * time_base.numerator() as f64
                        / time_base.denominator().max(1) as f64;
                    Duration::from_secs_f64(seconds.max(0.0))
                });

                let is_keyframe = packet.is_key();
                let size = packet.size();

                Some(Ok(PacketInfo {
                    stream_index,
                    pts,
                    dts,
                    pts_duration,
                    size,
                    is_keyframe,
                    time_base,
                }))
            }
            Err(FfmpegError::Eof) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(UnbundleError::from(e)))
            }
        }
    }
}
