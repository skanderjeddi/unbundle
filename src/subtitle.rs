//! Subtitle extraction.
//!
//! This module provides [`SubtitleExtractor`] for extracting text-based
//! subtitle tracks from media files, and [`SubtitleEntry`] for representing
//! individual subtitle events with timing information.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::MediaUnbundler;
//!
//! let mut unbundler = MediaUnbundler::open("input.mkv")?;
//! let entries = unbundler.subtitle().extract()?;
//! for entry in &entries {
//!     println!("[{:?} → {:?}] {}", entry.start_time, entry.end_time, entry.text);
//! }
//! # Ok::<(), unbundle::UnbundleError>(())
//! ```

use std::fmt;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use ffmpeg_next::{
    codec::context::Context as CodecContext,
    Subtitle,
    subtitle::Rect,
};

use crate::error::UnbundleError;
use crate::unbundler::MediaUnbundler;

/// A single subtitle event with timing and text content.
#[derive(Debug, Clone)]
pub struct SubtitleEntry {
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

impl fmt::Display for SubtitleFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubtitleFormat::Srt => write!(f, "SRT"),
            SubtitleFormat::WebVtt => write!(f, "WebVTT"),
            SubtitleFormat::Raw => write!(f, "Raw"),
        }
    }
}

/// Subtitle extraction operations.
///
/// Obtained via [`MediaUnbundler::subtitle`] or
/// [`MediaUnbundler::subtitle_track`]. Extracts text-based subtitle events
/// from the media file.
pub struct SubtitleExtractor<'a> {
    pub(crate) unbundler: &'a mut MediaUnbundler,
    /// Which subtitle stream to extract. `None` means "use default".
    pub(crate) stream_index: Option<usize>,
}

impl<'a> SubtitleExtractor<'a> {
    /// Resolve the subtitle stream index.
    fn resolve_stream_index(&self) -> Result<usize, UnbundleError> {
        self.stream_index
            .or(self.unbundler.subtitle_stream_index)
            .ok_or(UnbundleError::NoSubtitleStream)
    }

    /// Extract all subtitle entries from the stream.
    ///
    /// Returns a list of [`SubtitleEntry`] values sorted by start time.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoSubtitleStream`] if no subtitle stream exists.
    /// - [`UnbundleError::SubtitleDecodeError`] if decoding fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::MediaUnbundler;
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mkv")?;
    /// let entries = unbundler.subtitle().extract()?;
    /// println!("Found {} subtitle entries", entries.len());
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn extract(&mut self) -> Result<Vec<SubtitleEntry>, UnbundleError> {
        let subtitle_stream_index = self.resolve_stream_index()?;

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
                let pkt_pts = packet.pts().unwrap_or(0).max(0) as u64;
                let num = time_base.numerator() as u64;
                let den = time_base.denominator().max(1) as u64;
                pkt_pts * num * 1_000_000 / den
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
                    Rect::Text(t) => {
                        let s = t.get().trim().to_string();
                        if !s.is_empty() {
                            text_parts.push(s);
                        }
                    }
                    Rect::Ass(a) => {
                        let raw = a.get();
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
                entries.push(SubtitleEntry {
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
    /// Returns errors from [`extract`](SubtitleExtractor::extract) or
    /// I/O errors when writing the file.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaUnbundler, SubtitleFormat};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mkv")?;
    /// unbundler.subtitle().save("subtitles.srt", SubtitleFormat::Srt)?;
    /// # Ok::<(), unbundle::UnbundleError>(())
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
    /// Returns errors from [`extract`](SubtitleExtractor::extract).
    pub fn extract_text(
        &mut self,
        format: SubtitleFormat,
    ) -> Result<String, UnbundleError> {
        let entries = self.extract()?;
        Ok(format_subtitles(&entries, format))
    }
}

/// Format subtitle entries into a string in the given format.
fn format_subtitles(entries: &[SubtitleEntry], format: SubtitleFormat) -> String {
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
fn format_srt_timestamp(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    let millis = d.subsec_millis();
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

/// Format a duration as WebVTT timestamp (HH:MM:SS.mmm).
fn format_vtt_timestamp(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    let millis = d.subsec_millis();
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
        let mut start_idx = 0;
        for (i, c) in input.char_indices() {
            if c == ',' {
                comma_count += 1;
                if comma_count == 9 {
                    start_idx = i + 1;
                    break;
                }
            }
        }
        &input[start_idx..]
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
    result.replace("\\N", "\n").replace("\\n", "\n").trim().to_string()
}
