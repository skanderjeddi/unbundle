//! Media file validation.
//!
//! Provides [`MediaUnbundler::validate`] which inspects a media file and
//! returns a [`ValidationReport`] describing its structure and any potential
//! issues.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::MediaUnbundler;
//!
//! let unbundler = MediaUnbundler::open("input.mp4")?;
//! let report = unbundler.validate();
//! if report.is_valid() {
//!     println!("File is valid");
//! } else {
//!     for warning in &report.warnings {
//!         println!("Warning: {warning}");
//!     }
//! }
//! # Ok::<(), unbundle::UnbundleError>(())
//! ```

use std::fmt::{Display, Formatter, Result as FmtResult};
use std::time::Duration;

use crate::metadata::MediaMetadata;

/// Summary of media file validation.
///
/// Produced by [`MediaUnbundler::validate`](crate::MediaUnbundler::validate).
/// Contains lists of informational notices, warnings, and errors found during
/// validation.
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    /// Informational notices (not problems).
    pub info: Vec<String>,
    /// Non-fatal issues that may affect extraction quality.
    pub warnings: Vec<String>,
    /// Fatal issues that will prevent extraction.
    pub errors: Vec<String>,
}

impl ValidationReport {
    /// Returns `true` if no errors were found.
    ///
    /// Warnings do not affect this result — only errors make the report
    /// invalid.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Total number of issues (info + warnings + errors).
    pub fn issue_count(&self) -> usize {
        self.info.len() + self.warnings.len() + self.errors.len()
    }
}

impl Display for ValidationReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        for item in &self.info {
            writeln!(f, "[INFO] {item}")?;
        }
        for item in &self.warnings {
            writeln!(f, "[WARN] {item}")?;
        }
        for item in &self.errors {
            writeln!(f, "[ERROR] {item}")?;
        }
        if self.issue_count() == 0 {
            writeln!(f, "No issues found.")?;
        }
        Ok(())
    }
}

/// Run validation checks on the cached metadata.
///
/// This function is called by [`MediaUnbundler::validate`].
pub(crate) fn validate_metadata(metadata: &MediaMetadata) -> ValidationReport {
    let mut report = ValidationReport::default();

    // ── Stream presence ────────────────────────────────────────────
    if metadata.video.is_none() && metadata.audio.is_none() {
        report
            .errors
            .push("File contains neither video nor audio streams".to_string());
    }

    if metadata.video.is_none() {
        report.info.push("No video stream found".to_string());
    }

    if metadata.audio.is_none() {
        report.info.push("No audio stream found".to_string());
    }

    // ── Duration ───────────────────────────────────────────────────
    if metadata.duration == Duration::ZERO {
        report
            .warnings
            .push("Media duration is zero — frame/time-based extraction may fail".to_string());
    }

    // ── Video checks ───────────────────────────────────────────────
    if let Some(video) = &metadata.video {
        if video.width == 0 || video.height == 0 {
            report.errors.push(format!(
                "Invalid video dimensions: {}×{}",
                video.width, video.height,
            ));
        }

        if video.frames_per_second <= 0.0 {
            report.warnings.push(
                "Video frame rate is zero or negative — frame counting will be unreliable"
                    .to_string(),
            );
        } else if video.frames_per_second > 240.0 {
            report.warnings.push(format!(
                "Unusually high frame rate ({:.1} fps) — extraction may be slow",
                video.frames_per_second,
            ));
        }

        if video.frame_count == 0 && metadata.duration > Duration::ZERO {
            report
                .warnings
                .push("Estimated frame count is zero despite non-zero duration".to_string());
        }

        report.info.push(format!(
            "Video: {} {}×{} @ {:.2} fps, ~{} frames",
            video.codec, video.width, video.height, video.frames_per_second, video.frame_count,
        ));
    }

    // ── Audio checks ───────────────────────────────────────────────
    if let Some(audio) = &metadata.audio {
        if audio.sample_rate == 0 {
            report
                .errors
                .push("Audio sample rate is zero".to_string());
        }

        if audio.channels == 0 {
            report
                .errors
                .push("Audio channel count is zero".to_string());
        }

        report.info.push(format!(
            "Audio: {} {}Hz {}ch",
            audio.codec, audio.sample_rate, audio.channels,
        ));
    }

    // ── Multi-track info ───────────────────────────────────────────
    if let Some(tracks) = &metadata.audio_tracks {
        if tracks.len() > 1 {
            report.info.push(format!(
                "{} audio tracks available",
                tracks.len(),
            ));
        }
    }

    // ── Subtitle info ──────────────────────────────────────────────
    if let Some(sub) = &metadata.subtitle {
        let lang = sub
            .language
            .as_deref()
            .unwrap_or("unknown language");
        report.info.push(format!(
            "Subtitle: {} ({})",
            sub.codec, lang,
        ));
    }

    if let Some(tracks) = &metadata.subtitle_tracks {
        if tracks.len() > 1 {
            report.info.push(format!(
                "{} subtitle tracks available",
                tracks.len(),
            ));
        }
    }

    report
}
