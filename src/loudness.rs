//! Audio loudness analysis.
//!
//! This module provides [`LoudnessInfo`] for computing loudness-related
//! statistics from an audio stream. It decodes to mono f32, then computes
//! peak amplitude, RMS loudness, and derives an approximate dBFS value.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::MediaUnbundler;
//!
//! let mut unbundler = MediaUnbundler::open("input.mp4")?;
//! let loudness = unbundler.audio().analyze_loudness()?;
//! println!("Peak: {:.2} dBFS, RMS: {:.2} dBFS",
//!     loudness.peak_dbfs, loudness.rms_dbfs);
//! # Ok::<(), unbundle::UnbundleError>(())
//! ```

use std::time::Duration;

use ffmpeg_next::ChannelLayout;
use ffmpeg_next::codec::context::Context as CodecContext;
use ffmpeg_next::format::{Sample, sample::Type as SampleType};
use ffmpeg_next::frame::Audio as AudioFrame;
use ffmpeg_next::software::resampling::Context as ResamplingContext;

use crate::error::UnbundleError;
use crate::unbundler::MediaUnbundler;

/// Audio loudness statistics.
#[derive(Debug, Clone, Copy)]
pub struct LoudnessInfo {
    /// Peak sample amplitude (linear, 0.0–1.0).
    pub peak: f32,
    /// Peak in dBFS (decibels relative to full scale). 0.0 dBFS = maximum.
    pub peak_dbfs: f64,
    /// Root-mean-square amplitude (linear).
    pub rms: f32,
    /// RMS in dBFS.
    pub rms_dbfs: f64,
    /// Duration of the analyzed audio.
    pub duration: Duration,
    /// Total number of mono samples analyzed.
    pub total_samples: u64,
}

/// Decode audio to mono f32 and compute loudness statistics.
pub(crate) fn analyze_loudness_impl(
    unbundler: &mut MediaUnbundler,
    audio_stream_index: usize,
) -> Result<LoudnessInfo, UnbundleError> {
    log::debug!("Analyzing loudness (stream={})", audio_stream_index);
    let stream = unbundler
        .input_context
        .stream(audio_stream_index)
        .ok_or(UnbundleError::NoAudioStream)?;

    let codec_parameters = stream.parameters();
    let decoder_context = CodecContext::from_parameters(codec_parameters)?;
    let mut decoder = decoder_context.decoder().audio().map_err(|e| {
        UnbundleError::LoudnessError(format!("Failed to create audio decoder: {e}"))
    })?;

    let sample_rate = decoder.rate();

    let mut resampler = ResamplingContext::get(
        decoder.format(),
        decoder.channel_layout(),
        sample_rate,
        Sample::F32(SampleType::Packed),
        ChannelLayout::MONO,
        sample_rate,
    )
    .map_err(|e| {
        UnbundleError::LoudnessError(format!("Failed to create resampler: {e}"))
    })?;

    let mut peak: f32 = 0.0;
    let mut sum_sq: f64 = 0.0;
    let mut total_samples: u64 = 0;
    let mut decoded_frame = AudioFrame::empty();
    let mut resampled_frame = AudioFrame::empty();

    for (stream, packet) in unbundler.input_context.packets() {
        if stream.index() != audio_stream_index {
            continue;
        }

        decoder.send_packet(&packet).map_err(|e| {
            UnbundleError::LoudnessError(format!("Audio decode error: {e}"))
        })?;

        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            let _ = resampler.run(&decoded_frame, &mut resampled_frame).map_err(|e| {
                UnbundleError::LoudnessError(format!("Resample error: {e}"))
            })?;

            let data = resampled_frame.data(0);
            let sample_count = resampled_frame.samples();
            let float_samples: &[f32] = unsafe {
                std::slice::from_raw_parts(data.as_ptr() as *const f32, sample_count)
            };

            for &s in float_samples {
                let abs = s.abs();
                if abs > peak {
                    peak = abs;
                }
                sum_sq += (s as f64) * (s as f64);
            }
            total_samples += sample_count as u64;
        }
    }

    let rms = if total_samples > 0 {
        (sum_sq / total_samples as f64).sqrt() as f32
    } else {
        0.0
    };

    let peak_dbfs = if peak > 0.0 {
        20.0 * (peak as f64).log10()
    } else {
        f64::NEG_INFINITY
    };

    let rms_dbfs = if rms > 0.0 {
        20.0 * (rms as f64).log10()
    } else {
        f64::NEG_INFINITY
    };

    let duration = Duration::from_secs_f64(total_samples as f64 / sample_rate as f64);

    Ok(LoudnessInfo {
        peak,
        peak_dbfs,
        rms,
        rms_dbfs,
        duration,
        total_samples,
    })
}
