//! Subtitle extraction.
//!
//! This module provides [`SubtitleHandle`] for extracting text-based
//! subtitle tracks from media files, and [`SubtitleEvent`] for representing
//! individual subtitle events with timing information.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{MediaFile, UnbundleError};
//!
//! let mut unbundler = MediaFile::open("input.mkv")?;
//! let entries = unbundler.subtitle().extract()?;
//! for entry in &entries {
//!     println!("[{:?} → {:?}] {}", entry.start_time, entry.end_time, entry.text);
//! }
//! # Ok::<(), UnbundleError>(())
//! ```

use std::ffi::CString;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use ffmpeg_next::{
    Rational, Subtitle,
    codec::{Id, context::Context as CodecContext},
    packet::Mut as PacketMut,
    subtitle::{Bitmap as SubtitleBitmap, Rect},
};
use ffmpeg_sys_next::{AVFormatContext, AVRational};
use image::{DynamicImage, RgbaImage};

use crate::configuration::ExtractOptions;
use crate::error::UnbundleError;
use crate::unbundle::MediaFile;

/// A single subtitle event with timing and text content.
#[derive(Debug, Clone)]
pub struct SubtitleEvent {
    /// When this subtitle starts displaying.
    pub start_time: Duration,
    /// When this subtitle stops displaying.
    pub end_time: Duration,
    /// The text content of the subtitle. ASS formatting tags are stripped
    /// for [`SubtitleFormat::Srt`] and [`SubtitleFormat::WebVtt`] output.
    pub text: String,
    /// The zero-based index of this subtitle in the stream.
    pub index: usize,
}

/// Output format for saved subtitle files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    /// SubRip Text (.srt).
    Srt,
    /// Web Video Text Tracks (.vtt).
    WebVtt,
    /// Raw text, one entry per line with timestamps.
    Raw,
}

impl Display for SubtitleFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            SubtitleFormat::Srt => write!(f, "SRT"),
            SubtitleFormat::WebVtt => write!(f, "WebVTT"),
            SubtitleFormat::Raw => write!(f, "Raw"),
        }
    }
}

/// Subtitle extraction operations.
///
/// Obtained via [`MediaFile::subtitle`] or
/// [`MediaFile::subtitle_track`]. Extracts text-based subtitle events
/// from the media file.
pub struct SubtitleHandle<'a> {
    pub(crate) unbundler: &'a mut MediaFile,
    /// Which subtitle stream to extract. `None` means "use default".
    pub(crate) stream_index: Option<usize>,
}

impl<'a> SubtitleHandle<'a> {
    /// Resolve the subtitle stream index.
    fn resolve_stream_index(&self) -> Result<usize, UnbundleError> {
        self.stream_index
            .or(self.unbundler.subtitle_stream_index)
            .ok_or(UnbundleError::NoSubtitleStream)
    }

    /// Extract all subtitle entries from the stream.
    ///
    /// Returns a list of [`SubtitleEvent`] values sorted by start time.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoSubtitleStream`] if no subtitle stream exists.
    /// - [`UnbundleError::SubtitleDecodeError`] if decoding fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mkv")?;
    /// let entries = unbundler.subtitle().extract()?;
    /// println!("Found {} subtitle entries", entries.len());
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn extract(&mut self) -> Result<Vec<SubtitleEvent>, UnbundleError> {
        let subtitle_stream_index = self.resolve_stream_index()?;
        log::debug!("Extracting subtitles from stream {}", subtitle_stream_index);

        let stream = self
            .unbundler
            .input_context
            .stream(subtitle_stream_index)
            .ok_or(UnbundleError::NoSubtitleStream)?;

        let time_base = stream.time_base();
        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let mut decoder = decoder_context.decoder().subtitle().map_err(|e| {
            UnbundleError::SubtitleDecodeError(format!("Failed to create subtitle decoder: {e}"))
        })?;

        let mut entries = Vec::new();
        let mut entry_index: usize = 0;
        let mut subtitle = Subtitle::new();

        for (stream, packet) in self.unbundler.input_context.packets() {
            if stream.index() != subtitle_stream_index {
                continue;
            }

            let got_subtitle = decoder.decode(&packet, &mut subtitle).map_err(|e| {
                UnbundleError::SubtitleDecodeError(format!("Subtitle decode error: {e}"))
            })?;

            if !got_subtitle {
                continue;
            }

            // Compute base PTS in microseconds from the packet PTS.
            let base_pts_us = if let Some(pts) = subtitle.pts() {
                // pts is in AV_TIME_BASE (microseconds).
                pts.max(0) as u64
            } else {
                // Fall back to packet PTS converted via time base.
                let packet_pts = packet.pts().unwrap_or(0).max(0) as u64;
                let numerator = time_base.numerator() as u64;
                let denominator = time_base.denominator().max(1) as u64;
                packet_pts * numerator * 1_000_000 / denominator
            };

            let start_offset_ms = subtitle.start() as u64;
            let end_offset_ms = subtitle.end() as u64;

            let start_time =
                Duration::from_micros(base_pts_us) + Duration::from_millis(start_offset_ms);
            let end_time =
                Duration::from_micros(base_pts_us) + Duration::from_millis(end_offset_ms);

            // Collect text from all rects.
            let mut text_parts: Vec<String> = Vec::new();

            for rect in subtitle.rects() {
                match rect {
                    Rect::Text(text_ref) => {
                        let subtitle_text = text_ref.get().trim().to_string();
                        if !subtitle_text.is_empty() {
                            text_parts.push(subtitle_text);
                        }
                    }
                    Rect::Ass(ass_ref) => {
                        let raw = ass_ref.get();
                        let cleaned = strip_ass_tags(raw);
                        if !cleaned.is_empty() {
                            text_parts.push(cleaned);
                        }
                    }
                    _ => {
                        // Bitmap subtitles are not supported as text.
                    }
                }
            }

            if !text_parts.is_empty() {
                entries.push(SubtitleEvent {
                    start_time,
                    end_time,
                    text: text_parts.join("\n"),
                    index: entry_index,
                });
                entry_index += 1;
            }
        }

        entries.sort_by_key(|e| e.start_time);
        Ok(entries)
    }

    /// Extract subtitles and save them to a file.
    ///
    /// Extracts all subtitle entries and writes them in the specified format.
    ///
    /// # Errors
    ///
    /// Returns errors from [`extract`](SubtitleHandle::extract) or
    /// I/O errors when writing the file.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, SubtitleFormat, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mkv")?;
    /// unbundler.subtitle().save("subtitles.srt", SubtitleFormat::Srt)?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn save<P: AsRef<Path>>(
        &mut self,
        path: P,
        format: SubtitleFormat,
    ) -> Result<(), UnbundleError> {
        let entries = self.extract()?;
        let content = format_subtitles(&entries, format);
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Extract subtitles and format them as a string.
    ///
    /// Convenience method that returns the formatted subtitle text
    /// without writing to a file.
    ///
    /// # Errors
    ///
    /// Returns errors from [`extract`](SubtitleHandle::extract).
    pub fn extract_text(&mut self, format: SubtitleFormat) -> Result<String, UnbundleError> {
        let entries = self.extract()?;
        Ok(format_subtitles(&entries, format))
    }

    /// Extract subtitle entries within a time range.
    ///
    /// Returns only the [`SubtitleEvent`] values whose display interval
    /// overlaps `[start, end)`. A subtitle is included when its start time
    /// is before `end` **and** its end time is after `start`.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::InvalidRange`] if `start >= end`.
    /// - Plus any errors from [`extract`](SubtitleHandle::extract).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mkv")?;
    /// let subs = unbundler
    ///     .subtitle()
    ///     .extract_range(Duration::from_secs(10), Duration::from_secs(30))?;
    /// println!("Found {} subtitles in range", subs.len());
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn extract_range(
        &mut self,
        start: Duration,
        end: Duration,
    ) -> Result<Vec<SubtitleEvent>, UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }

        let entries = self.extract()?;
        Ok(entries
            .into_iter()
            .filter(|e| e.start_time < end && e.end_time > start)
            .collect())
    }

    /// Extract subtitles in a time range and save to a file.
    ///
    /// Combines [`extract_range`](SubtitleHandle::extract_range) with
    /// file output in the specified format.
    ///
    /// # Errors
    ///
    /// Returns errors from [`extract_range`](SubtitleHandle::extract_range)
    /// or I/O errors when writing the file.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use unbundle::{MediaFile, SubtitleFormat, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mkv")?;
    /// unbundler.subtitle().save_range(
    ///     "partial.srt",
    ///     SubtitleFormat::Srt,
    ///     Duration::from_secs(0),
    ///     Duration::from_secs(60),
    /// )?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn save_range<P: AsRef<Path>>(
        &mut self,
        path: P,
        format: SubtitleFormat,
        start: Duration,
        end: Duration,
    ) -> Result<(), UnbundleError> {
        let entries = self.extract_range(start, end)?;
        let content = format_subtitles(&entries, format);
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Extract subtitles in a time range and format as a string.
    ///
    /// Combines [`extract_range`](SubtitleHandle::extract_range) with
    /// text formatting.
    ///
    /// # Errors
    ///
    /// Returns errors from [`extract_range`](SubtitleHandle::extract_range).
    pub fn extract_text_range(
        &mut self,
        format: SubtitleFormat,
        start: Duration,
        end: Duration,
    ) -> Result<String, UnbundleError> {
        let entries = self.extract_range(start, end)?;
        Ok(format_subtitles(&entries, format))
    }

    /// Search subtitle entries for text matching a pattern (case-insensitive).
    ///
    /// Returns all [`SubtitleEvent`] values whose text contains `query`
    /// (compared case-insensitively).
    ///
    /// # Errors
    ///
    /// Returns errors from [`extract`](SubtitleHandle::extract).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mkv")?;
    /// let results = unbundler.subtitle().search("hello")?;
    /// for sub in &results {
    ///     println!("[{:?}] {}", sub.start_time, sub.text);
    /// }
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn search(&mut self, query: &str) -> Result<Vec<SubtitleEvent>, UnbundleError> {
        let entries = self.extract()?;
        let query_lower = query.to_lowercase();
        Ok(entries
            .into_iter()
            .filter(|e| e.text.to_lowercase().contains(&query_lower))
            .collect())
    }

    /// Search subtitle entries for an exact text match (case-sensitive).
    ///
    /// Returns all [`SubtitleEvent`] values whose text contains `query`
    /// exactly (case-sensitive comparison).
    ///
    /// # Errors
    ///
    /// Returns errors from [`extract`](SubtitleHandle::extract).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mkv")?;
    /// let results = unbundler.subtitle().search_exact("Hello")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn search_exact(&mut self, query: &str) -> Result<Vec<SubtitleEvent>, UnbundleError> {
        let entries = self.extract()?;
        Ok(entries
            .into_iter()
            .filter(|e| e.text.contains(query))
            .collect())
    }

    /// Extract bitmap subtitle events as images.
    ///
    /// DVD, PGS, and DVB subtitle tracks use images rather than text.
    /// This method decodes those bitmap rects and converts each one into
    /// a [`BitmapSubtitleEvent`] containing an [`image::DynamicImage`]
    /// along with timing and positional metadata.
    ///
    /// Text-only subtitle rects are silently skipped.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoSubtitleStream`] if no subtitle stream exists.
    /// - [`UnbundleError::SubtitleDecodeError`] if decoding fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mkv")?;
    /// let bitmaps = unbundler.subtitle().extract_bitmaps()?;
    /// for (i, bmp) in bitmaps.iter().enumerate() {
    ///     bmp.image.save(format!("sub_{i}.png")).unwrap();
    /// }
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn extract_bitmaps(&mut self) -> Result<Vec<BitmapSubtitleEvent>, UnbundleError> {
        let subtitle_stream_index = self.resolve_stream_index()?;
        log::debug!(
            "Extracting bitmap subtitles from stream {}",
            subtitle_stream_index
        );

        let stream = self
            .unbundler
            .input_context
            .stream(subtitle_stream_index)
            .ok_or(UnbundleError::NoSubtitleStream)?;

        let time_base = stream.time_base();
        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let mut decoder = decoder_context.decoder().subtitle().map_err(|e| {
            UnbundleError::SubtitleDecodeError(format!("Failed to create subtitle decoder: {e}"))
        })?;

        let mut events = Vec::new();
        let mut event_index: usize = 0;
        let mut subtitle = Subtitle::new();

        for (stream, packet) in self.unbundler.input_context.packets() {
            if stream.index() != subtitle_stream_index {
                continue;
            }

            let got_subtitle = decoder.decode(&packet, &mut subtitle).map_err(|e| {
                UnbundleError::SubtitleDecodeError(format!("Subtitle decode error: {e}"))
            })?;

            if !got_subtitle {
                continue;
            }

            let base_pts_us = if let Some(pts) = subtitle.pts() {
                pts.max(0) as u64
            } else {
                let packet_pts = packet.pts().unwrap_or(0).max(0) as u64;
                let numerator = time_base.numerator() as u64;
                let denominator = time_base.denominator().max(1) as u64;
                packet_pts * numerator * 1_000_000 / denominator
            };

            let start_offset_ms = subtitle.start() as u64;
            let end_offset_ms = subtitle.end() as u64;

            let start_time =
                Duration::from_micros(base_pts_us) + Duration::from_millis(start_offset_ms);
            let end_time =
                Duration::from_micros(base_pts_us) + Duration::from_millis(end_offset_ms);

            for rect in subtitle.rects() {
                if let Rect::Bitmap(ref bitmap) = rect {
                    if let Some(image) = decode_bitmap_rect(bitmap) {
                        events.push(BitmapSubtitleEvent {
                            start_time,
                            end_time,
                            x: bitmap.x() as u32,
                            y: bitmap.y() as u32,
                            image,
                            index: event_index,
                        });
                        event_index += 1;
                    }
                }
            }
        }

        events.sort_by_key(|e| e.start_time);
        Ok(events)
    }

    // ── Stream copy (lossless) ─────────────────────────────────────────

    /// Copy the subtitle stream verbatim to a file without re-encoding.
    ///
    /// Unlike [`save`](SubtitleHandle::save) which decodes subtitles and
    /// re-formats them as SRT/WebVTT, this copies packets directly from
    /// the input, preserving the original codec and timing. The output
    /// container format is inferred from the file extension.
    ///
    /// This is equivalent to `ffmpeg -i input.mkv -vn -an -c:s copy output.srt`.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoSubtitleStream`] if no subtitle stream exists.
    /// - [`UnbundleError::StreamCopyError`] if the output container does
    ///   not support the source codec.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mkv")?;
    /// unbundler.subtitle().stream_copy("output.srt")?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn stream_copy<P: AsRef<Path>>(&mut self, path: P) -> Result<(), UnbundleError> {
        self.copy_stream_to_file(path.as_ref(), None, None, None)
    }

    /// Copy a subtitle segment verbatim to a file without re-encoding.
    ///
    /// Like [`stream_copy`](SubtitleHandle::stream_copy) but copies only
    /// packets between `start` and `end`. Because there is no re-encoding,
    /// the actual boundaries are aligned to the nearest packet.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::InvalidRange`] if `start >= end`.
    /// - Plus any errors from [`stream_copy`](SubtitleHandle::stream_copy).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mkv")?;
    /// unbundler.subtitle().stream_copy_range(
    ///     "segment.srt",
    ///     Duration::from_secs(10),
    ///     Duration::from_secs(60),
    /// )?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn stream_copy_range<P: AsRef<Path>>(
        &mut self,
        path: P,
        start: Duration,
        end: Duration,
    ) -> Result<(), UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        self.copy_stream_to_file(path.as_ref(), Some(start), Some(end), None)
    }

    /// Copy the subtitle stream verbatim to a file with cancellation support.
    ///
    /// Like [`stream_copy`](SubtitleHandle::stream_copy) but accepts an
    /// [`ExtractOptions`] for cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested,
    /// or any error from [`stream_copy`](SubtitleHandle::stream_copy).
    pub fn stream_copy_with_options<P: AsRef<Path>>(
        &mut self,
        path: P,
        config: &ExtractOptions,
    ) -> Result<(), UnbundleError> {
        self.copy_stream_to_file(path.as_ref(), None, None, Some(config))
    }

    /// Copy a subtitle segment verbatim to a file with cancellation support.
    ///
    /// Like [`stream_copy_range`](SubtitleHandle::stream_copy_range) but
    /// accepts an [`ExtractOptions`].
    pub fn stream_copy_range_with_options<P: AsRef<Path>>(
        &mut self,
        path: P,
        start: Duration,
        end: Duration,
        config: &ExtractOptions,
    ) -> Result<(), UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        self.copy_stream_to_file(path.as_ref(), Some(start), Some(end), Some(config))
    }

    /// Copy the subtitle stream verbatim to memory without re-encoding.
    ///
    /// `container_format` is the FFmpeg short name for the output container
    /// (e.g. `"matroska"` for MKV, `"srt"` for SubRip).
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoSubtitleStream`] if no subtitle stream exists.
    /// - [`UnbundleError::StreamCopyError`] if the container format is
    ///   invalid or does not support the source codec.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mkv")?;
    /// let bytes = unbundler.subtitle().stream_copy_to_memory("srt")?;
    /// println!("Copied {} bytes", bytes.len());
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn stream_copy_to_memory(
        &mut self,
        container_format: &str,
    ) -> Result<Vec<u8>, UnbundleError> {
        self.copy_stream_to_memory(container_format, None, None, None)
    }

    /// Copy a subtitle segment verbatim to memory without re-encoding.
    ///
    /// Like [`stream_copy_to_memory`](SubtitleHandle::stream_copy_to_memory) but
    /// copies only packets between `start` and `end`.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::InvalidRange`] if `start >= end`.
    /// - Plus any errors from [`stream_copy_to_memory`](SubtitleHandle::stream_copy_to_memory).
    pub fn stream_copy_range_to_memory(
        &mut self,
        container_format: &str,
        start: Duration,
        end: Duration,
    ) -> Result<Vec<u8>, UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        self.copy_stream_to_memory(container_format, Some(start), Some(end), None)
    }

    // ── Stream copy (lossless) helpers ──────────────────────────────

    /// Copy the subtitle stream verbatim to a file without decoding or
    /// re-encoding. Container format is inferred from the file extension.
    fn copy_stream_to_file(
        &mut self,
        path: &Path,
        start: Option<Duration>,
        end: Option<Duration>,
        config: Option<&ExtractOptions>,
    ) -> Result<(), UnbundleError> {
        let subtitle_stream_index = self.resolve_stream_index()?;
        log::debug!(
            "Stream-copying subtitle to file {:?} (stream={})",
            path,
            subtitle_stream_index
        );

        let stream = self
            .unbundler
            .input_context
            .stream(subtitle_stream_index)
            .ok_or(UnbundleError::NoSubtitleStream)?;
        let input_time_base = stream.time_base();

        // Create output context — container format inferred from extension.
        let mut output_context = ffmpeg_next::format::output(&path).map_err(|error| {
            UnbundleError::StreamCopyError(format!("Failed to create output: {error}"))
        })?;

        // Add an output stream with the same codec parameters (stream copy).
        {
            let mut out_stream = output_context
                .add_stream(ffmpeg_next::encoder::find(Id::None))
                .map_err(|error| {
                    UnbundleError::StreamCopyError(format!("Failed to add stream: {error}"))
                })?;
            out_stream.set_parameters(stream.parameters());
            // Let the muxer choose the correct codec tag.
            unsafe {
                (*out_stream.parameters().as_mut_ptr()).codec_tag = 0;
            }
        }

        output_context.write_header().map_err(|error| {
            UnbundleError::StreamCopyError(format!("Failed to write header: {error}"))
        })?;

        // Seek to start position if specified.
        if let Some(start_time) = start {
            let seek_timestamp = crate::conversion::duration_to_seek_timestamp(start_time);
            self.unbundler
                .input_context
                .seek(seek_timestamp, ..seek_timestamp)?;
        }

        let end_stream_timestamp = end.map(|end_time| {
            crate::conversion::duration_to_stream_timestamp(end_time, input_time_base)
        });

        let output_time_base = output_context.stream(0).unwrap().time_base();

        // Copy packets.
        for (stream, mut packet) in self.unbundler.input_context.packets() {
            if let Some(active_config) = config
                && active_config.is_cancelled()
            {
                return Err(UnbundleError::Cancelled);
            }
            if stream.index() != subtitle_stream_index {
                continue;
            }

            if let Some(end_ts) = end_stream_timestamp
                && let Some(pts) = packet.pts()
                && pts > end_ts
            {
                break;
            }

            packet.set_stream(0);
            packet.rescale_ts(input_time_base, output_time_base);
            packet.set_position(-1);
            packet
                .write_interleaved(&mut output_context)
                .map_err(|error| {
                    UnbundleError::StreamCopyError(format!("Failed to write packet: {error}"))
                })?;
        }

        output_context.write_trailer().map_err(|error| {
            UnbundleError::StreamCopyError(format!("Failed to write trailer: {error}"))
        })?;

        Ok(())
    }

    /// Copy the subtitle stream verbatim to an in-memory buffer using
    /// FFmpeg's dynamic buffer I/O. No decoding or re-encoding.
    fn copy_stream_to_memory(
        &mut self,
        container_format: &str,
        start: Option<Duration>,
        end: Option<Duration>,
        config: Option<&ExtractOptions>,
    ) -> Result<Vec<u8>, UnbundleError> {
        let subtitle_stream_index = self.resolve_stream_index()?;
        log::debug!(
            "Stream-copying subtitle to memory (format={}, stream={})",
            container_format,
            subtitle_stream_index
        );

        let stream = self
            .unbundler
            .input_context
            .stream(subtitle_stream_index)
            .ok_or(UnbundleError::NoSubtitleStream)?;
        let input_time_base = stream.time_base();
        let codec_parameters = stream.parameters();

        // Seek to start position if specified.
        if let Some(start_time) = start {
            let seek_timestamp = crate::conversion::duration_to_seek_timestamp(start_time);
            self.unbundler
                .input_context
                .seek(seek_timestamp, ..seek_timestamp)?;
        }

        let end_stream_timestamp = end.map(|end_time| {
            crate::conversion::duration_to_stream_timestamp(end_time, input_time_base)
        });

        // ── In-memory muxing via avio_open_dyn_buf ─────────────────
        //
        // SAFETY: Same pattern as audio stream copy. We allocate a muxer
        // context backed by a dynamically-growing memory buffer, copy
        // packets verbatim (no decode/encode), then extract the buffer.

        unsafe {
            let container_name_c = CString::new(container_format).map_err(|error| {
                UnbundleError::StreamCopyError(format!("Invalid container format name: {error}"))
            })?;

            let mut output_format_context: *mut AVFormatContext = std::ptr::null_mut();
            let allocation_result = ffmpeg_sys_next::avformat_alloc_output_context2(
                &mut output_format_context,
                std::ptr::null_mut(),
                container_name_c.as_ptr(),
                std::ptr::null(),
            );
            if allocation_result < 0 || output_format_context.is_null() {
                return Err(UnbundleError::StreamCopyError(
                    "Failed to allocate output format context".to_string(),
                ));
            }

            // Open dynamic buffer for I/O.
            let dynamic_buffer_result =
                ffmpeg_sys_next::avio_open_dyn_buf(&mut (*output_format_context).pb);
            if dynamic_buffer_result < 0 {
                ffmpeg_sys_next::avformat_free_context(output_format_context);
                return Err(UnbundleError::StreamCopyError(
                    "Failed to open dynamic buffer".to_string(),
                ));
            }

            // Add an output stream.
            let output_stream =
                ffmpeg_sys_next::avformat_new_stream(output_format_context, std::ptr::null());
            if output_stream.is_null() {
                let mut buffer_pointer: *mut u8 = std::ptr::null_mut();
                ffmpeg_sys_next::avio_close_dyn_buf(
                    (*output_format_context).pb,
                    &mut buffer_pointer,
                );
                if !buffer_pointer.is_null() {
                    ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                }
                (*output_format_context).pb = std::ptr::null_mut();
                ffmpeg_sys_next::avformat_free_context(output_format_context);
                return Err(UnbundleError::StreamCopyError(
                    "Failed to add output stream".to_string(),
                ));
            }

            // Copy codec parameters from input stream (no re-encode).
            ffmpeg_sys_next::avcodec_parameters_copy(
                (*output_stream).codecpar,
                codec_parameters.as_ptr(),
            );
            (*(*output_stream).codecpar).codec_tag = 0;

            (*output_stream).time_base = AVRational {
                num: input_time_base.numerator(),
                den: input_time_base.denominator(),
            };

            // Write the container header.
            let write_header_result =
                ffmpeg_sys_next::avformat_write_header(output_format_context, std::ptr::null_mut());
            if write_header_result < 0 {
                let mut buffer_pointer: *mut u8 = std::ptr::null_mut();
                ffmpeg_sys_next::avio_close_dyn_buf(
                    (*output_format_context).pb,
                    &mut buffer_pointer,
                );
                if !buffer_pointer.is_null() {
                    ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                }
                (*output_format_context).pb = std::ptr::null_mut();
                ffmpeg_sys_next::avformat_free_context(output_format_context);
                return Err(UnbundleError::StreamCopyError(
                    "Failed to write output header".to_string(),
                ));
            }

            // Retrieve output time base (may differ after header is written).
            let output_time_base = Rational::new(
                (*output_stream).time_base.num,
                (*output_stream).time_base.den,
            );

            // Copy packets.
            for (stream, mut packet) in self.unbundler.input_context.packets() {
                if let Some(active_config) = config
                    && active_config.is_cancelled()
                {
                    let mut buffer_pointer: *mut u8 = std::ptr::null_mut();
                    ffmpeg_sys_next::avio_close_dyn_buf(
                        (*output_format_context).pb,
                        &mut buffer_pointer,
                    );
                    if !buffer_pointer.is_null() {
                        ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                    }
                    (*output_format_context).pb = std::ptr::null_mut();
                    ffmpeg_sys_next::avformat_free_context(output_format_context);
                    return Err(UnbundleError::Cancelled);
                }

                if stream.index() != subtitle_stream_index {
                    continue;
                }

                if let Some(end_ts) = end_stream_timestamp
                    && let Some(pts) = packet.pts()
                    && pts > end_ts
                {
                    break;
                }

                packet.set_stream(0);
                packet.rescale_ts(input_time_base, output_time_base);
                packet.set_position(-1);
                ffmpeg_sys_next::av_interleaved_write_frame(
                    output_format_context,
                    packet.as_mut_ptr(),
                );
            }

            // Write the container trailer.
            ffmpeg_sys_next::av_write_trailer(output_format_context);

            // Extract the dynamic buffer contents.
            let mut buffer_pointer: *mut u8 = std::ptr::null_mut();
            let buffer_size = ffmpeg_sys_next::avio_close_dyn_buf(
                (*output_format_context).pb,
                &mut buffer_pointer,
            );

            let result_bytes = if buffer_size > 0 && !buffer_pointer.is_null() {
                std::slice::from_raw_parts(buffer_pointer, buffer_size as usize).to_vec()
            } else {
                Vec::new()
            };

            if !buffer_pointer.is_null() {
                ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
            }

            // Prevent the destructor from calling avio_close on the freed buffer.
            (*output_format_context).pb = std::ptr::null_mut();
            ffmpeg_sys_next::avformat_free_context(output_format_context);

            Ok(result_bytes)
        }
    }
}

/// A bitmap subtitle event containing an image and timing.
#[derive(Debug, Clone)]
pub struct BitmapSubtitleEvent {
    /// When this subtitle starts displaying.
    pub start_time: Duration,
    /// When this subtitle stops displaying.
    pub end_time: Duration,
    /// Horizontal position on the video frame.
    pub x: u32,
    /// Vertical position on the video frame.
    pub y: u32,
    /// The decoded subtitle image (RGBA).
    pub image: DynamicImage,
    /// Zero-based index of this event.
    pub index: usize,
}

/// Decode a PAL8 bitmap subtitle rect into an RGBA [`DynamicImage`].
fn decode_bitmap_rect(bitmap: &SubtitleBitmap<'_>) -> Option<DynamicImage> {
    let width = bitmap.width();
    let height = bitmap.height();
    if width == 0 || height == 0 {
        return None;
    }

    let color_count = bitmap.colors();

    // Safety: we access the raw AVSubtitleRect to read data[0] (pixel indices)
    // and data[1] (RGBA palette). This is the only way on FFmpeg 5.0+.
    unsafe {
        let pointer = bitmap.as_ptr();
        let pixel_data = (*pointer).data[0]; // pixel indices
        let palette_data = (*pointer).data[1]; // RGBA palette
        let linesize = (*pointer).linesize[0] as usize;

        if pixel_data.is_null() || palette_data.is_null() {
            return None;
        }

        // Read palette (up to 256 RGBA entries).
        let palette_length = color_count.min(256);
        let palette_bytes =
            std::slice::from_raw_parts(palette_data as *const u8, palette_length * 4);

        let mut rgba_buffer = vec![0u8; (width * height * 4) as usize];

        for row in 0..height as usize {
            for column in 0..width as usize {
                let palette_index = *pixel_data.add(row * linesize + column) as usize;
                let destination_offset = (row * width as usize + column) * 4;
                if palette_index < palette_length {
                    rgba_buffer[destination_offset] = palette_bytes[palette_index * 4];
                    rgba_buffer[destination_offset + 1] = palette_bytes[palette_index * 4 + 1];
                    rgba_buffer[destination_offset + 2] = palette_bytes[palette_index * 4 + 2];
                    rgba_buffer[destination_offset + 3] = palette_bytes[palette_index * 4 + 3];
                }
                // else: leave as transparent black (0,0,0,0).
            }
        }

        let rgba_image = RgbaImage::from_raw(width, height, rgba_buffer)?;
        Some(DynamicImage::ImageRgba8(rgba_image))
    }
}

/// Format subtitle entries into a string in the given format.
fn format_subtitles(entries: &[SubtitleEvent], format: SubtitleFormat) -> String {
    let mut output = Vec::new();

    match format {
        SubtitleFormat::Srt => {
            for (i, entry) in entries.iter().enumerate() {
                writeln!(output, "{}", i + 1).unwrap();
                writeln!(
                    output,
                    "{} --> {}",
                    format_srt_timestamp(entry.start_time),
                    format_srt_timestamp(entry.end_time),
                )
                .unwrap();
                writeln!(output, "{}", entry.text).unwrap();
                writeln!(output).unwrap();
            }
        }
        SubtitleFormat::WebVtt => {
            writeln!(output, "WEBVTT").unwrap();
            writeln!(output).unwrap();
            for (i, entry) in entries.iter().enumerate() {
                writeln!(output, "{}", i + 1).unwrap();
                writeln!(
                    output,
                    "{} --> {}",
                    format_vtt_timestamp(entry.start_time),
                    format_vtt_timestamp(entry.end_time),
                )
                .unwrap();
                writeln!(output, "{}", entry.text).unwrap();
                writeln!(output).unwrap();
            }
        }
        SubtitleFormat::Raw => {
            for entry in entries {
                writeln!(
                    output,
                    "[{:?} → {:?}] {}",
                    entry.start_time, entry.end_time, entry.text
                )
                .unwrap();
            }
        }
    }

    String::from_utf8(output).unwrap_or_default()
}

/// Format a duration as SRT timestamp (HH:MM:SS,mmm).
fn format_srt_timestamp(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    let millis = duration.subsec_millis();
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

/// Format a duration as WebVTT timestamp (HH:MM:SS.mmm).
fn format_vtt_timestamp(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    let millis = duration.subsec_millis();
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

/// Strip ASS/SSA formatting tags from a string.
///
/// Removes `{\...}` style override blocks and the `Dialogue:` prefix
/// common in ASS subtitle data.
fn strip_ass_tags(input: &str) -> String {
    // ASS dialogue lines often have the format:
    // Dialogue: 0,0:00:01.00,0:00:04.00,Default,,0,0,0,,Text here
    // The actual text is after the last comma in the comma-separated fields.
    let text = if input.starts_with("Dialogue:") {
        // Find the 9th comma (text starts after it).
        let mut comma_count = 0;
        let mut start_index = 0;
        for (i, c) in input.char_indices() {
            if c == ',' {
                comma_count += 1;
                if comma_count == 9 {
                    start_index = i + 1;
                    break;
                }
            }
        }
        &input[start_index..]
    } else {
        input
    };

    // Remove {\...} override blocks.
    let mut result = String::with_capacity(text.len());
    let mut in_tag = false;

    for c in text.chars() {
        if c == '{' && !in_tag {
            in_tag = true;
        } else if c == '}' && in_tag {
            in_tag = false;
        } else if !in_tag {
            result.push(c);
        }
    }

    // Replace \N (ASS line break) with newline.
    result
        .replace("\\N", "\n")
        .replace("\\n", "\n")
        .trim()
        .to_string()
}
