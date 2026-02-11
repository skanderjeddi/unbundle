//! Parallel video frame extraction.
//!
//! This module provides [`parallel_extract_frames`] which distributes frame
//! decoding across multiple threads using [`rayon`]. Each worker opens its
//! own demuxer and decoder so there is no shared mutable state.
//!
//! The public API is exposed through
//! [`VideoExtractor::frames_parallel`](crate::VideoExtractor) — this module
//! contains only the internal implementation.

use std::path::{Path, PathBuf};

use image::DynamicImage;
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use crate::config::ExtractionConfig;
use crate::error::UnbundleError;
use crate::metadata::VideoMetadata;
use crate::unbundler::MediaUnbundler;
use crate::video::FrameRange;

/// Extract frames in parallel by splitting work across rayon threads.
///
/// Each worker opens its own file context and decodes a contiguous sub-range
/// of frames. Results are collected and returned in frame-number order.
///
/// # Arguments
///
/// * `file_path` — Path to the media file.
/// * `frame_numbers` — Sorted, deduplicated frame numbers to extract.
/// * `video_metadata` — Cached video metadata (used for validation only).
/// * `config` — Extraction settings forwarded to each worker.
pub(crate) fn parallel_extract_frames(
    file_path: &PathBuf,
    frame_numbers: &[u64],
    _video_metadata: &VideoMetadata,
    config: &ExtractionConfig,
) -> Result<Vec<(u64, DynamicImage)>, UnbundleError> {
    if frame_numbers.is_empty() {
        return Ok(Vec::new());
    }

    // Split into contiguous runs. A "run" is a sequence where each frame
    // is at most `gap_threshold` frames from the next — these are cheaper
    // to decode sequentially than to seek to individually.
    let chunks = split_into_runs(frame_numbers, 30);

    let path = file_path.clone();
    let cfg = config.clone();

    let results: Result<Vec<Vec<(u64, DynamicImage)>>, UnbundleError> = chunks
        .into_par_iter()
        .map(|chunk| {
            if cfg.is_cancelled() {
                return Err(UnbundleError::Cancelled);
            }
            decode_chunk(&path, &chunk, &cfg)
        })
        .collect();

    let mut all_frames: Vec<(u64, DynamicImage)> =
        results?.into_iter().flatten().collect();
    all_frames.sort_by_key(|(num, _)| *num);
    Ok(all_frames)
}

/// Split a sorted list of frame numbers into contiguous "runs" where
/// consecutive elements differ by at most `gap_threshold`.
fn split_into_runs(frame_numbers: &[u64], gap_threshold: u64) -> Vec<Vec<u64>> {
    if frame_numbers.is_empty() {
        return Vec::new();
    }

    let mut runs: Vec<Vec<u64>> = Vec::new();
    let mut current_run: Vec<u64> = vec![frame_numbers[0]];

    for &num in &frame_numbers[1..] {
        if num - *current_run.last().unwrap() <= gap_threshold {
            current_run.push(num);
        } else {
            runs.push(std::mem::take(&mut current_run));
            current_run.push(num);
        }
    }

    if !current_run.is_empty() {
        runs.push(current_run);
    }

    runs
}

/// Decode a chunk of frame numbers from a fresh file context.
fn decode_chunk(
    file_path: &Path,
    frame_numbers: &[u64],
    config: &ExtractionConfig,
) -> Result<Vec<(u64, DynamicImage)>, UnbundleError> {
    let mut unbundler = MediaUnbundler::open(file_path)?;
    let mut frames = Vec::with_capacity(frame_numbers.len());

    // Use for_each_frame_with_config with Specific to leverage sequential
    // decode optimisation within each chunk.
    let range = FrameRange::Specific(frame_numbers.to_vec());
    unbundler
        .video()
        .for_each_frame_with_config(range, config, |frame_number, image| {
            frames.push((frame_number, image));
            Ok(())
        })?;

    Ok(frames)
}
