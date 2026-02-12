//! Audio waveform generation.
//!
//! This module provides [`WaveformOptions`] and [`WaveformData`] for
//! generating waveform data suitable for visualisation. Audio samples
//! are decoded, downmixed to mono, and bucketed into a configurable
//! number of bins, with min/max/RMS values per bin.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{MediaFile, UnbundleError, WaveformOptions};
//!
//! let mut unbundler = MediaFile::open("input.mp4")?;
//! let config = WaveformOptions::new().bins(800);
//! let waveform = unbundler.audio().generate_waveform(&config)?;
//! println!("Bins: {}", waveform.bins.len());
//! # Ok::<(), UnbundleError>(())
//! ```

use std::time::Duration;

use ffmpeg_next::{ChannelLayout, Rational};
use ffmpeg_next::codec::context::Context as CodecContext;
use ffmpeg_next::format::{Sample, sample::Type as SampleType};
use ffmpeg_next::frame::Audio as AudioFrame;
use ffmpeg_next::software::resampling::Context as ResamplingContext;

use crate::error::UnbundleError;
use crate::unbundle::MediaFile;

/// Configuration for waveform generation.
#[derive(Debug, Clone)]
pub struct WaveformOptions {
    /// Number of output bins (columns). Default: 800.
    pub bins: usize,
    /// Optional start time to limit the range.
    pub start: Option<Duration>,
    /// Optional end time to limit the range.
    pub end: Option<Duration>,
}

impl Default for WaveformOptions {
    fn default() -> Self {
        Self {
            bins: 800,
            start: None,
            end: None,
        }
    }
}

impl WaveformOptions {
    /// Create a new [`WaveformOptions`] with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the number of output bins.
    pub fn bins(mut self, bins: usize) -> Self {
        self.bins = bins;
        self
    }

    /// Set an optional start time.
    pub fn start(mut self, start: Duration) -> Self {
        self.start = Some(start);
        self
    }

    /// Set an optional end time.
    pub fn end(mut self, end: Duration) -> Self {
        self.end = Some(end);
        self
    }
}

/// A single waveform bin containing amplitude statistics.
#[derive(Debug, Clone, Copy)]
pub struct WaveformBin {
    /// Minimum sample value in this bin (range −1.0..1.0).
    pub min: f32,
    /// Maximum sample value in this bin (range −1.0..1.0).
    pub max: f32,
    /// Root-mean-square amplitude for this bin.
    pub rms: f32,
}

/// Waveform data produced by [`AudioHandle::generate_waveform`](crate::AudioHandle).
#[derive(Debug, Clone)]
pub struct WaveformData {
    /// One entry per bin.
    pub bins: Vec<WaveformBin>,
    /// The total duration of audio that was analyzed.
    pub duration: Duration,
    /// The sample rate of the decoded audio.
    pub sample_rate: u32,
    /// Total number of mono samples decoded.
    pub total_samples: u64,
}

/// Decode audio to mono f32, bucket into bins, compute min/max/rms per bin.
pub(crate) fn generate_waveform_impl(
    unbundler: &mut MediaFile,
    audio_stream_index: usize,
    config: &WaveformOptions,
) -> Result<WaveformData, UnbundleError> {
    log::debug!("Generating waveform (stream={}, bins={})", audio_stream_index, config.bins);
    let stream = unbundler
        .input_context
        .stream(audio_stream_index)
        .ok_or(UnbundleError::NoAudioStream)?;

    let time_base: Rational = stream.time_base();
    let codec_parameters = stream.parameters();
    let decoder_context = CodecContext::from_parameters(codec_parameters)?;
    let mut decoder = decoder_context.decoder().audio().map_err(|e| {
        UnbundleError::WaveformDecodeError(format!("Failed to create audio decoder: {e}"))
    })?;

    let sample_rate = decoder.rate();

    // Set up resampler: convert to mono f32.
    let mut resampler = ResamplingContext::get(
        decoder.format(),
        decoder.channel_layout(),
        sample_rate,
        Sample::F32(SampleType::Packed),
        ChannelLayout::MONO,
        sample_rate,
    )
    .map_err(|e| {
        UnbundleError::WaveformDecodeError(format!("Failed to create resampler: {e}"))
    })?;

    // Compute time-range boundaries in stream time base.
    let start_pts: Option<i64> = config.start.map(|d| {
        (d.as_secs_f64() * time_base.denominator() as f64 / time_base.numerator().max(1) as f64)
            as i64
    });
    let end_pts: Option<i64> = config.end.map(|d| {
        (d.as_secs_f64() * time_base.denominator() as f64 / time_base.numerator().max(1) as f64)
            as i64
    });

    // Collect all mono f32 samples.
    let mut all_samples: Vec<f32> = Vec::new();
    let mut decoded_frame = AudioFrame::empty();
    let mut resampled_frame = AudioFrame::empty();

    for (stream, packet) in unbundler.input_context.packets() {
        if stream.index() != audio_stream_index {
            continue;
        }

        // Time-range filtering at the packet level.
        if let Some(end) = end_pts {
            if let Some(pkt_pts) = packet.pts() {
                if pkt_pts > end {
                    break;
                }
            }
        }
        if let Some(start) = start_pts {
            if let Some(pkt_pts) = packet.pts() {
                // Skip packets clearly before the start. Their decoded
                // samples may still overlap, but this is a coarse filter.
                if let Some(dur) = packet.duration().checked_add(pkt_pts as i64) {
                    if dur < start {
                        continue;
                    }
                }
            }
        }

        decoder.send_packet(&packet).map_err(|e| {
            UnbundleError::WaveformDecodeError(format!("Audio decode error: {e}"))
        })?;

        while decoder.receive_frame(&mut decoded_frame).is_ok() {
            let delay = resampler.run(&decoded_frame, &mut resampled_frame).map_err(|e| {
                UnbundleError::WaveformDecodeError(format!("Resample error: {e}"))
            })?;

            let data = resampled_frame.data(0);
            let sample_count = resampled_frame.samples();
            let float_samples: &[f32] = unsafe {
                std::slice::from_raw_parts(data.as_ptr() as *const f32, sample_count)
            };
            all_samples.extend_from_slice(float_samples);

            if delay.is_some() {
                // Flush the remaining samples from the resampler.
                let flush_frame = AudioFrame::empty();
                if resampler.run(&flush_frame, &mut resampled_frame).is_ok() {
                    let data = resampled_frame.data(0);
                    let sc = resampled_frame.samples();
                    let fs: &[f32] = unsafe {
                        std::slice::from_raw_parts(data.as_ptr() as *const f32, sc)
                    };
                    all_samples.extend_from_slice(fs);
                }
            }
        }
    }

    let total_samples = all_samples.len() as u64;
    let duration = Duration::from_secs_f64(total_samples as f64 / sample_rate as f64);

    // Bucket into bins.
    let num_bins = config.bins.max(1);
    let samples_per_bin = (all_samples.len() as f64 / num_bins as f64).ceil() as usize;

    let mut bins = Vec::with_capacity(num_bins);
    for chunk in all_samples.chunks(samples_per_bin.max(1)) {
        let mut min_val = f32::INFINITY;
        let mut max_val = f32::NEG_INFINITY;
        let mut sum_sq = 0.0_f64;

        for &s in chunk {
            if s < min_val {
                min_val = s;
            }
            if s > max_val {
                max_val = s;
            }
            sum_sq += (s as f64) * (s as f64);
        }

        let rms = (sum_sq / chunk.len() as f64).sqrt() as f32;
        bins.push(WaveformBin {
            min: min_val,
            max: max_val,
            rms,
        });
    }

    // Pad to exactly num_bins if the last chunks were short.
    while bins.len() < num_bins {
        bins.push(WaveformBin {
            min: 0.0,
            max: 0.0,
            rms: 0.0,
        });
    }

    Ok(WaveformData {
        bins,
        duration,
        sample_rate,
        total_samples,
    })
}
