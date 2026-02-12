//! Lazy pull-based audio sample iteration.
//!
//! This module provides [`AudioIterator`] for streaming decoded audio
//! samples without collecting the entire track into memory. Audio is
//! decoded, resampled to mono f32, and yielded in chunks.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::MediaUnbundler;
//!
//! let mut unbundler = MediaUnbundler::open("input.mp4")?;
//! let iter = unbundler.audio().sample_iter()?;
//! for result in iter {
//!     let chunk = result?;
//!     println!("Got {} samples at {:?}", chunk.samples.len(), chunk.timestamp);
//! }
//! # Ok::<(), unbundle::UnbundleError>(())
//! ```

use std::time::Duration;

use ffmpeg_next::{ChannelLayout, Error as FfmpegError, Packet};
use ffmpeg_next::codec::context::Context as CodecContext;
use ffmpeg_next::format::{Sample, sample::Type as SampleType};
use ffmpeg_next::frame::Audio as AudioFrame;
use ffmpeg_next::software::resampling::Context as ResamplingContext;

use crate::error::UnbundleError;
use crate::unbundler::MediaUnbundler;

/// A chunk of decoded audio samples.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Mono f32 samples in this chunk.
    pub samples: Vec<f32>,
    /// Approximate timestamp of the first sample in this chunk.
    pub timestamp: Duration,
    /// Sample rate of the decoded audio.
    pub sample_rate: u32,
}

/// A lazy iterator over decoded audio samples.
///
/// Yields [`AudioChunk`] values containing mono f32 samples. Each chunk
/// corresponds roughly to one decoded audio frame.
pub struct AudioIterator<'a> {
    unbundler: &'a mut MediaUnbundler,
    decoder: ffmpeg_next::decoder::Audio,
    resampler: ResamplingContext,
    audio_stream_index: usize,
    sample_rate: u32,
    samples_yielded: u64,
    decoded_frame: AudioFrame,
    resampled_frame: AudioFrame,
    eof_sent: bool,
    done: bool,
}

impl<'a> AudioIterator<'a> {
    /// Create a new audio iterator for the given stream index.
    pub(crate) fn new(
        unbundler: &'a mut MediaUnbundler,
        audio_stream_index: usize,
    ) -> Result<Self, UnbundleError> {
        log::debug!("Creating AudioIterator (stream={})", audio_stream_index);
        let stream = unbundler
            .input_context
            .stream(audio_stream_index)
            .ok_or(UnbundleError::NoAudioStream)?;

        let codec_parameters = stream.parameters();
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let decoder = decoder_context.decoder().audio().map_err(|e| {
            UnbundleError::AudioDecodeError(format!("Failed to create audio decoder: {e}"))
        })?;

        let sample_rate = decoder.rate();

        let resampler = ResamplingContext::get(
            decoder.format(),
            decoder.channel_layout(),
            sample_rate,
            Sample::F32(SampleType::Packed),
            ChannelLayout::MONO,
            sample_rate,
        )
        .map_err(|e| {
            UnbundleError::AudioDecodeError(format!("Failed to create resampler: {e}"))
        })?;

        Ok(Self {
            unbundler,
            decoder,
            resampler,
            audio_stream_index,
            sample_rate,
            samples_yielded: 0,
            decoded_frame: AudioFrame::empty(),
            resampled_frame: AudioFrame::empty(),
            eof_sent: false,
            done: false,
        })
    }
}

impl<'a> Iterator for AudioIterator<'a> {
    type Item = Result<AudioChunk, UnbundleError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        loop {
            // Try to receive a decoded frame.
            if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
                match self
                    .resampler
                    .run(&self.decoded_frame, &mut self.resampled_frame)
                {
                    Ok(_) => {
                        let data = self.resampled_frame.data(0);
                        let sample_count = self.resampled_frame.samples();
                        let float_samples: &[f32] = unsafe {
                            std::slice::from_raw_parts(
                                data.as_ptr() as *const f32,
                                sample_count,
                            )
                        };

                        let timestamp = Duration::from_secs_f64(
                            self.samples_yielded as f64 / self.sample_rate as f64,
                        );

                        self.samples_yielded += sample_count as u64;

                        return Some(Ok(AudioChunk {
                            samples: float_samples.to_vec(),
                            timestamp,
                            sample_rate: self.sample_rate,
                        }));
                    }
                    Err(e) => {
                        self.done = true;
                        return Some(Err(UnbundleError::AudioDecodeError(format!(
                            "Resample error: {e}"
                        ))));
                    }
                }
            }

            // Feed more packets.
            if self.eof_sent {
                self.done = true;
                return None;
            }

            let mut packet = Packet::empty();
            match packet.read(&mut self.unbundler.input_context) {
                Ok(()) => {
                    if packet.stream() as usize == self.audio_stream_index {
                        if let Err(e) = self.decoder.send_packet(&packet) {
                            self.done = true;
                            return Some(Err(UnbundleError::from(e)));
                        }
                    }
                }
                Err(FfmpegError::Eof) => {
                    if let Err(e) = self.decoder.send_eof() {
                        self.done = true;
                        return Some(Err(UnbundleError::from(e)));
                    }
                    self.eof_sent = true;
                }
                Err(_) => {
                    // Non-fatal read error — try next packet.
                }
            }
        }
    }
}
