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

use std::fmt::{Display, Formatter, Result as FmtResult};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use ffmpeg_next::{
    Subtitle,
    codec::context::Context as CodecContext,
    subtitle::{Bitmap as SubtitleBitmap, Rect},
};
use image::{DynamicImage, RgbaImage};

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
