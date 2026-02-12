//! Audio extraction.
//!
//! This module provides [`AudioHandle`] for extracting audio tracks from
//! media files, and [`AudioFormat`] for specifying the output encoding.
//! Audio can be extracted to memory as `Vec<u8>` or written directly to a file.

use std::{ffi::CString, fmt::{Display, Formatter, Result as FmtResult}, path::Path, time::Duration};

use ffmpeg_next::{
    ChannelLayout,
    codec::{context::Context as CodecContext, Id},
    decoder::Audio as AudioDecoder,
    encoder::Audio as AudioEncoder,
    format::{context::Output, Sample, sample::Type as SampleType},
    frame::Audio as AudioFrame,
    Packet,
    packet::Mut as PacketMut,
    Rational,
    software::resampling::Context as ResamplingContext,
};
use ffmpeg_sys_next::{AVFormatContext, AVRational};

use crate::{configuration::ExtractOptions, error::UnbundleError, unbundle::MediaFile};
use crate::audio_iterator::AudioIterator;

#[cfg(feature = "loudness")]
use crate::loudness::LoudnessInfo;

#[cfg(feature = "async")]
use crate::stream::AudioFuture;

#[cfg(feature = "waveform")]
use crate::waveform::{WaveformData, WaveformOptions};

/// Audio output format.
///
/// Determines the container format and codec used when encoding extracted
/// audio data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// WAV (PCM signed 16-bit little-endian). Lossless, universally supported.
    Wav,
    /// MP3 (MPEG Audio Layer III). Lossy, widely supported. Requires libmp3lame.
    Mp3,
    /// FLAC (Free Lossless Audio Codec). Lossless, good compression.
    Flac,
    /// AAC (Advanced Audio Coding). Lossy, high quality at low bitrates.
    Aac,
}

impl Display for AudioFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            AudioFormat::Wav => write!(f, "WAV"),
            AudioFormat::Mp3 => write!(f, "MP3"),
            AudioFormat::Flac => write!(f, "FLAC"),
            AudioFormat::Aac => write!(f, "AAC"),
        }
    }
}

impl AudioFormat {
    /// Return the FFmpeg container format name for this audio format.
    fn container_name(&self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Flac => "flac",
            AudioFormat::Aac => "adts",
        }
    }

    /// Return the FFmpeg codec ID for this audio format.
    fn codec_id(&self) -> Id {
        match self {
            AudioFormat::Wav => Id::PCM_S16LE,
            AudioFormat::Mp3 => Id::MP3,
            AudioFormat::Flac => Id::FLAC,
            AudioFormat::Aac => Id::AAC,
        }
    }
}

/// Audio extraction operations.
///
/// Obtained via [`MediaFile::audio`] or
/// [`MediaFile::audio_track`]. Provides methods for extracting
/// complete audio tracks or segments, either to memory or to files.
pub struct AudioHandle<'a> {
    pub(crate) unbundler: &'a mut MediaFile,
    /// Which audio stream to extract. `None` means "use default".
    pub(crate) stream_index: Option<usize>,
}

impl<'a> AudioHandle<'a> {
    /// Resolve the audio stream index, falling back to the unbundler's default.
    fn resolve_stream_index(&self) -> Result<usize, UnbundleError> {
        self.stream_index
            .or(self.unbundler.audio_stream_index)
            .ok_or(UnbundleError::NoAudioStream)
    }

    /// Extract the complete audio track to memory.
    ///
    /// Returns the encoded audio data as a byte vector in the specified format.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoAudioStream`] if the file has no audio stream.
    /// - [`UnbundleError::AudioDecodeError`] or [`UnbundleError::AudioEncodeError`]
    ///   if transcoding fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{AudioFormat, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let audio_bytes = unbundler.audio().extract(AudioFormat::Wav)?;
    /// println!("Extracted {} bytes", audio_bytes.len());
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn extract(&mut self, format: AudioFormat) -> Result<Vec<u8>, UnbundleError> {
        self.extract_audio_to_memory(format, None, None, None)
    }

    /// Extract an audio segment by time range to memory.
    ///
    /// Extracts audio between `start` and `end` timestamps (inclusive).
    ///
    /// # Errors
    ///
    /// Returns errors from [`extract`](AudioHandle::extract), plus
    /// [`UnbundleError::InvalidTimestamp`] if either timestamp exceeds the
    /// media duration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use unbundle::{AudioFormat, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let segment = unbundler.audio().extract_range(
    ///     Duration::from_secs(10),
    ///     Duration::from_secs(20),
    ///     AudioFormat::Mp3,
    /// )?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn extract_range(
        &mut self,
        start: Duration,
        end: Duration,
        format: AudioFormat,
    ) -> Result<Vec<u8>, UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        self.extract_audio_to_memory(format, Some(start), Some(end), None)
    }

    /// Save the complete audio track to a file.
    ///
    /// The output format is determined by the `format` parameter, not the file
    /// extension.
    ///
    /// # Errors
    ///
    /// Returns errors from audio decoding/encoding, plus I/O errors if the
    /// output file cannot be created.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{AudioFormat, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// unbundler.audio().save("output.wav", AudioFormat::Wav)?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn save<P: AsRef<Path>>(
        &mut self,
        path: P,
        format: AudioFormat,
    ) -> Result<(), UnbundleError> {
        self.save_audio_to_file(path.as_ref(), format, None, None, None)
    }

    /// Save an audio segment to a file.
    ///
    /// # Errors
    ///
    /// Returns errors from [`save`](AudioHandle::save), plus
    /// [`UnbundleError::InvalidTimestamp`] if either timestamp exceeds the
    /// media duration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use unbundle::{AudioFormat, MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// unbundler.audio().save_range(
    ///     "segment.mp3",
    ///     Duration::from_secs(30),
    ///     Duration::from_secs(60),
    ///     AudioFormat::Mp3,
    /// )?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn save_range<P: AsRef<Path>>(
        &mut self,
        path: P,
        start: Duration,
        end: Duration,
        format: AudioFormat,
    ) -> Result<(), UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        self.save_audio_to_file(path.as_ref(), format, Some(start), Some(end), None)
    }

    /// Extract the complete audio track to memory with cancellation support.
    ///
    /// Like [`extract`](AudioHandle::extract) but accepts an
    /// [`ExtractOptions`] for cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested, or
    /// any error from [`extract`](AudioHandle::extract).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{AudioFormat, CancellationToken, ExtractOptions, MediaFile, UnbundleError};
    ///
    /// let token = CancellationToken::new();
    /// let config = ExtractOptions::new()
    ///     .with_cancellation(token.clone());
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let audio = unbundler.audio().extract_with_options(AudioFormat::Wav, &config)?;
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn extract_with_options(
        &mut self,
        format: AudioFormat,
        config: &ExtractOptions,
    ) -> Result<Vec<u8>, UnbundleError> {
        self.extract_audio_to_memory(format, None, None, Some(config))
    }

    /// Extract an audio segment to memory with cancellation support.
    ///
    /// Like [`extract_range`](AudioHandle::extract_range) but accepts an
    /// [`ExtractOptions`].
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested, or
    /// any error from [`extract_range`](AudioHandle::extract_range).
    pub fn extract_range_with_options(
        &mut self,
        start: Duration,
        end: Duration,
        format: AudioFormat,
        config: &ExtractOptions,
    ) -> Result<Vec<u8>, UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        self.extract_audio_to_memory(format, Some(start), Some(end), Some(config))
    }

    /// Save the complete audio track to a file with cancellation support.
    ///
    /// Like [`save`](AudioHandle::save) but accepts an
    /// [`ExtractOptions`].
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested, or
    /// any error from [`save`](AudioHandle::save).
    pub fn save_with_options<P: AsRef<Path>>(
        &mut self,
        path: P,
        format: AudioFormat,
        config: &ExtractOptions,
    ) -> Result<(), UnbundleError> {
        self.save_audio_to_file(path.as_ref(), format, None, None, Some(config))
    }

    /// Save an audio segment to a file with cancellation support.
    ///
    /// Like [`save_range`](AudioHandle::save_range) but accepts an
    /// [`ExtractOptions`].
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::Cancelled`] if cancellation is requested, or
    /// any error from [`save_range`](AudioHandle::save_range).
    pub fn save_range_with_options<P: AsRef<Path>>(
        &mut self,
        path: P,
        start: Duration,
        end: Duration,
        format: AudioFormat,
        config: &ExtractOptions,
    ) -> Result<(), UnbundleError> {
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        self.save_audio_to_file(path.as_ref(), format, Some(start), Some(end), Some(config))
    }

    /// Generate waveform data from the audio stream.
    ///
    /// Decodes audio to mono, buckets samples into the configured number
    /// of bins, and computes min/max/RMS amplitude per bin. The result is
    /// suitable for rendering a visual waveform.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoAudioStream`] if no audio stream exists.
    /// - [`UnbundleError::WaveformDecodeError`] if decoding fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError, WaveformOptions};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let waveform = unbundler.audio().generate_waveform(
    ///     &WaveformOptions::new().bins(1000),
    /// )?;
    /// println!("Waveform bins: {}", waveform.bins.len());
    /// # Ok::<(), UnbundleError>(())
    /// ```
    #[cfg(feature = "waveform")]
    pub fn generate_waveform(
        &mut self,
        config: &WaveformOptions,
    ) -> Result<WaveformData, UnbundleError> {
        let audio_stream_index = self.resolve_stream_index()?;
        crate::waveform::generate_waveform_impl(self.unbundler, audio_stream_index, config)
    }

    /// Analyze loudness of the audio stream.
    ///
    /// Decodes the entire audio track to mono and computes peak amplitude,
    /// RMS level, and their dBFS equivalents.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoAudioStream`] if no audio stream exists.
    /// - [`UnbundleError::LoudnessError`] if decoding fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let loudness = unbundler.audio().analyze_loudness()?;
    /// println!("Peak: {:.1} dBFS", loudness.peak_dbfs);
    /// # Ok::<(), UnbundleError>(())
    /// ```
    #[cfg(feature = "loudness")]
    pub fn analyze_loudness(
        &mut self,
    ) -> Result<LoudnessInfo, UnbundleError> {
        let audio_stream_index = self.resolve_stream_index()?;
        crate::loudness::analyze_loudness_impl(self.unbundler, audio_stream_index)
    }

    /// Create a lazy iterator over decoded audio samples.
    ///
    /// The iterator yields [`AudioChunk`](crate::AudioChunk) values
    /// containing mono f32 samples. Each chunk corresponds roughly to
    /// one decoded audio frame, so the caller processes audio
    /// incrementally without loading the entire track into memory.
    ///
    /// The iterator borrows the unbundler mutably; drop it to release
    /// the borrow.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::NoAudioStream`] if no audio stream exists.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{MediaFile, UnbundleError};
    ///
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let iter = unbundler.audio().sample_iter()?;
    /// let mut total = 0u64;
    /// for chunk in iter {
    ///     total += chunk?.samples.len() as u64;
    /// }
    /// println!("Total mono samples: {total}");
    /// # Ok::<(), UnbundleError>(())
    /// ```
    pub fn sample_iter(self) -> Result<AudioIterator<'a>, UnbundleError> {
        let audio_stream_index = self.resolve_stream_index()?;
        AudioIterator::new(self.unbundler, audio_stream_index)
    }

    // ── Private helpers ────────────────────────────────────────────────

    /// Extract audio to an in-memory buffer using FFmpeg's dynamic buffer I/O.
    ///
    /// This uses `avio_open_dyn_buf` / `avio_close_dyn_buf` from the FFmpeg C
    /// API (via `ffmpeg_sys_next`) to mux encoded audio into a memory buffer
    /// without touching the filesystem.
    fn extract_audio_to_memory(
        &mut self,
        format: AudioFormat,
        start: Option<Duration>,
        end: Option<Duration>,
        config: Option<&ExtractOptions>,
    ) -> Result<Vec<u8>, UnbundleError> {
        let audio_stream_index = self.resolve_stream_index()?;
        log::debug!("Extracting audio to memory (format={}, stream={})", format, audio_stream_index);

        // Validate timestamps.
        let media_duration = self.unbundler.metadata.duration;
        if let Some(start_time) = start
            && start_time > media_duration
        {
            return Err(UnbundleError::InvalidTimestamp(start_time));
        }
        if let Some(end_time) = end
            && end_time > media_duration
        {
            return Err(UnbundleError::InvalidTimestamp(end_time));
        }

        // Gather stream info before entering the unsafe block.
        let stream = self
            .unbundler
            .input_context
            .stream(audio_stream_index)
            .ok_or(UnbundleError::NoAudioStream)?;
        let input_time_base = stream.time_base();
        let codec_parameters = stream.parameters();

        // Create decoder.
        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let mut decoder = decoder_context
            .decoder()
            .audio()
            .map_err(|error| UnbundleError::AudioDecodeError(error.to_string()))?;

        let input_sample_rate = decoder.rate();
        let input_channel_layout = decoder.channel_layout();

        // Determine encoder settings.
        let output_codec = ffmpeg_next::encoder::find(format.codec_id())
            .ok_or(UnbundleError::UnsupportedAudioFormat(format))?;

        // Pick a sample format supported by the encoder.
        let output_sample_format = output_codec
            .audio()
            .ok()
            .and_then(|audio_codec| audio_codec.formats())
            .and_then(|mut formats| formats.next())
            .unwrap_or(Sample::I16(SampleType::Packed));

        let output_sample_rate = input_sample_rate;
        let output_channel_layout = input_channel_layout;

        // Seek to start position if a range is specified.
        if let Some(start_time) = start {
            let start_timestamp =
                crate::conversion::duration_to_stream_timestamp(start_time, input_time_base);
            self.unbundler
                .input_context
                .seek(start_timestamp, ..start_timestamp)?;
        }

        // Compute end timestamp in stream time base for range filtering.
        let end_stream_timestamp = end.map(|end_time| {
            crate::conversion::duration_to_stream_timestamp(end_time, input_time_base)
        });

        // ── In-memory muxing via avio_open_dyn_buf ─────────────────
        //
        // SAFETY: We use raw FFmpeg C API calls to create an output format
        // context backed by a dynamically-growing memory buffer. The sequence
        // is:
        //   1. avformat_alloc_output_context2  — allocate muxer context
        //   2. avio_open_dyn_buf               — attach a memory-backed I/O
        //   3. add stream, write header, write packets, write trailer
        //   4. avio_close_dyn_buf              — extract the buffer
        //   5. null out pb, then free the context
        //
        // We null out `(*output_format_context).pb` before freeing the context
        // to prevent FFmpeg from calling `avio_close` on the already-freed
        // dynamic buffer.

        unsafe {
            let container_name = format.container_name();
            let container_name_c = CString::new(container_name).map_err(|error| {
                UnbundleError::AudioEncodeError(format!("Invalid container format name: {error}"))
            })?;

            let mut output_format_context: *mut AVFormatContext = std::ptr::null_mut();
            let allocation_result = ffmpeg_sys_next::avformat_alloc_output_context2(
                &mut output_format_context,
                std::ptr::null_mut(),
                container_name_c.as_ptr(),
                std::ptr::null(),
            );
            if allocation_result < 0 || output_format_context.is_null() {
                return Err(UnbundleError::AudioEncodeError(
                    "Failed to allocate output format context".to_string(),
                ));
            }

            // Open dynamic buffer for I/O.
            let dynamic_buffer_result =
                ffmpeg_sys_next::avio_open_dyn_buf(&mut (*output_format_context).pb);
            if dynamic_buffer_result < 0 {
                ffmpeg_sys_next::avformat_free_context(output_format_context);
                return Err(UnbundleError::AudioEncodeError(
                    "Failed to open dynamic buffer for audio output".to_string(),
                ));
            }

            // Add an output audio stream.
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
                return Err(UnbundleError::AudioEncodeError(
                    "Failed to add output stream".to_string(),
                ));
            }

            // Set up the encoder via the safe API (we'll copy parameters back).
            let encoder_result = self.create_audio_encoder(
                format,
                output_sample_format,
                output_sample_rate,
                output_channel_layout,
            );

            let (mut encoder, encoder_time_base) = match encoder_result {
                Ok(value) => value,
                Err(error) => {
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
                    return Err(error);
                }
            };

            // Copy encoder parameters to the output stream.
            ffmpeg_sys_next::avcodec_parameters_from_context(
                (*output_stream).codecpar,
                encoder.as_ptr(),
            );
            (*output_stream).time_base = AVRational {
                num: encoder_time_base.numerator(),
                den: encoder_time_base.denominator(),
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
                return Err(UnbundleError::AudioEncodeError(
                    "Failed to write output header".to_string(),
                ));
            }

            // Set up resampler if the decoder and encoder sample formats differ.
            let mut resampler = ResamplingContext::get(
                decoder.format(),
                decoder.channel_layout(),
                decoder.rate(),
                output_sample_format,
                output_channel_layout,
                output_sample_rate,
            )
            .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;

            // Decode → resample → encode → write loop.
            let mut decoded_audio_frame = AudioFrame::empty();
            let mut resampled_frame = AudioFrame::empty();
            let mut encoded_packet = Packet::empty();
            let mut samples_written: i64 = 0;
            let mut writer = MemoryPacketWriter { format_context: output_format_context };

            let transcode_result = self.transcode_audio_packets(
                audio_stream_index,
                &mut decoder,
                &mut resampler,
                &mut encoder,
                &mut decoded_audio_frame,
                &mut resampled_frame,
                &mut encoded_packet,
                &mut samples_written,
                encoder_time_base,
                end_stream_timestamp,
                config,
                &mut writer,
            );

            if let Err(error) = transcode_result {
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
                return Err(error);
            }

            // Flush the decoder.
            let _ = decoder.send_eof();
            while decoder.receive_frame(&mut decoded_audio_frame).is_ok() {
                if let Err(error) = resample_encode_write(
                    &mut resampler,
                    &mut encoder,
                    &decoded_audio_frame,
                    &mut resampled_frame,
                    &mut encoded_packet,
                    &mut samples_written,
                    encoder_time_base,
                    &mut writer,
                ) {
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
                    return Err(error);
                }
            }

            // Flush the encoder.
            let _ = encoder.send_eof();
            while encoder.receive_packet(&mut encoded_packet).is_ok() {
                encoded_packet.set_stream(0);
                encoded_packet.rescale_ts(encoder_time_base, encoder_time_base);
                let write_result = ffmpeg_sys_next::av_interleaved_write_frame(
                    output_format_context,
                    encoded_packet.as_mut_ptr(),
                );
                if write_result < 0 {
                    break;
                }
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

    /// Save audio to a file using the safe `ffmpeg_next::format::output` API.
    fn save_audio_to_file(
        &mut self,
        path: &Path,
        format: AudioFormat,
        start: Option<Duration>,
        end: Option<Duration>,
        config: Option<&ExtractOptions>,
    ) -> Result<(), UnbundleError> {
        let audio_stream_index = self.resolve_stream_index()?;
        log::debug!("Saving audio to file {:?} (format={}, stream={})", path, format, audio_stream_index);

        let media_duration = self.unbundler.metadata.duration;
        if let Some(start_time) = start
            && start_time > media_duration
        {
            return Err(UnbundleError::InvalidTimestamp(start_time));
        }
        if let Some(end_time) = end
            && end_time > media_duration
        {
            return Err(UnbundleError::InvalidTimestamp(end_time));
        }

        let stream = self
            .unbundler
            .input_context
            .stream(audio_stream_index)
            .ok_or(UnbundleError::NoAudioStream)?;
        let input_time_base = stream.time_base();
        let codec_parameters = stream.parameters();

        let decoder_context = CodecContext::from_parameters(codec_parameters)?;
        let mut decoder = decoder_context
            .decoder()
            .audio()
            .map_err(|error| UnbundleError::AudioDecodeError(error.to_string()))?;

        let input_sample_rate = decoder.rate();
        let input_channel_layout = decoder.channel_layout();

        let output_codec = ffmpeg_next::encoder::find(format.codec_id())
            .ok_or(UnbundleError::UnsupportedAudioFormat(format))?;

        let output_sample_format = output_codec
            .audio()
            .ok()
            .and_then(|audio_codec| audio_codec.formats())
            .and_then(|mut formats| formats.next())
            .unwrap_or(Sample::I16(SampleType::Packed));

        let output_sample_rate = input_sample_rate;
        let output_channel_layout = input_channel_layout;

        // Seek if a start time was specified.
        if let Some(start_time) = start {
            let start_timestamp =
                crate::conversion::duration_to_stream_timestamp(start_time, input_time_base);
            self.unbundler
                .input_context
                .seek(start_timestamp, ..start_timestamp)?;
        }

        let end_stream_timestamp = end.map(|end_time| {
            crate::conversion::duration_to_stream_timestamp(end_time, input_time_base)
        });

        // Create output context via the safe API.
        let mut output_context = ffmpeg_next::format::output_as(&path, format.container_name())
            .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;

        let (mut encoder, encoder_time_base) = self.create_audio_encoder(
            format,
            output_sample_format,
            output_sample_rate,
            output_channel_layout,
        )?;

        // Add output stream and set parameters.
        {
            let mut output_stream = output_context.add_stream(output_codec)?;
            output_stream.set_parameters(&encoder);
            output_stream.set_time_base(encoder_time_base);
        }

        output_context
            .write_header()
            .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;

        let mut resampler = ResamplingContext::get(
            decoder.format(),
            decoder.channel_layout(),
            decoder.rate(),
            output_sample_format,
            output_channel_layout,
            output_sample_rate,
        )
        .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;

        let mut decoded_audio_frame = AudioFrame::empty();
        let mut resampled_frame = AudioFrame::empty();
        let mut encoded_packet = Packet::empty();
        let mut samples_written: i64 = 0;

        {
            let mut writer = FilePacketWriter { output_context: &mut output_context };

            // Decode → resample → encode → write loop.
            self.transcode_audio_packets(
                audio_stream_index,
                &mut decoder,
                &mut resampler,
                &mut encoder,
                &mut decoded_audio_frame,
                &mut resampled_frame,
                &mut encoded_packet,
                &mut samples_written,
                encoder_time_base,
                end_stream_timestamp,
                config,
                &mut writer,
            )?;

            // Flush decoder.
            let _ = decoder.send_eof();
            while decoder.receive_frame(&mut decoded_audio_frame).is_ok() {
                resample_encode_write(
                    &mut resampler,
                    &mut encoder,
                    &decoded_audio_frame,
                    &mut resampled_frame,
                    &mut encoded_packet,
                    &mut samples_written,
                    encoder_time_base,
                    &mut writer,
                )?;
            }

            // Flush encoder.
            let _ = encoder.send_eof();
            while encoder.receive_packet(&mut encoded_packet).is_ok() {
                encoded_packet.set_stream(0);
                encoded_packet.rescale_ts(encoder_time_base, encoder_time_base);
                writer.write_packet(&mut encoded_packet)?;
            }
        }

        output_context
            .write_trailer()
            .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;

        Ok(())
    }

    /// Create an audio encoder configured for the specified output format.
    fn create_audio_encoder(
        &self,
        format: AudioFormat,
        sample_format: Sample,
        sample_rate: u32,
        channel_layout: ChannelLayout,
    ) -> Result<(AudioEncoder, Rational), UnbundleError> {
        let output_codec = ffmpeg_next::encoder::find(format.codec_id())
            .ok_or(UnbundleError::UnsupportedAudioFormat(format))?;

        let mut encoder_context = CodecContext::new()
            .encoder()
            .audio()
            .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;

        encoder_context.set_rate(sample_rate as i32);
        encoder_context.set_channel_layout(channel_layout);
        encoder_context.set_format(sample_format);
        encoder_context.set_time_base(Rational(1, sample_rate as i32));

        // Set bit rate for lossy codecs.
        match format {
            AudioFormat::Mp3 | AudioFormat::Aac => {
                encoder_context.set_bit_rate(128_000);
            }
            AudioFormat::Wav | AudioFormat::Flac => {
                // Lossless — bit rate is determined by sample format and rate.
            }
        }

        let encoder = encoder_context
            .open_as(output_codec)
            .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;

        let time_base = Rational(1, sample_rate as i32);

        Ok((encoder, time_base))
    }

    /// Decode, resample, encode, and write audio packets to the given output.
    #[allow(clippy::too_many_arguments)]
    fn transcode_audio_packets<W: PacketWriter>(
        &mut self,
        audio_stream_index: usize,
        decoder: &mut AudioDecoder,
        resampler: &mut ResamplingContext,
        encoder: &mut AudioEncoder,
        decoded_audio_frame: &mut AudioFrame,
        resampled_frame: &mut AudioFrame,
        encoded_packet: &mut Packet,
        samples_written: &mut i64,
        encoder_time_base: Rational,
        end_stream_timestamp: Option<i64>,
        config: Option<&ExtractOptions>,
        writer: &mut W,
    ) -> Result<(), UnbundleError> {
        for (stream, packet) in self.unbundler.input_context.packets() {
            if let Some(cfg) = config
                && cfg.is_cancelled()
            {
                return Err(UnbundleError::Cancelled);
            }
            if stream.index() != audio_stream_index {
                continue;
            }

            if let Some(end_timestamp) = end_stream_timestamp
                && let Some(packet_pts) = packet.pts()
                && packet_pts > end_timestamp
            {
                break;
            }

            decoder
                .send_packet(&packet)
                .map_err(|error| UnbundleError::AudioDecodeError(error.to_string()))?;

            while decoder.receive_frame(decoded_audio_frame).is_ok() {
                if let Some(end_timestamp) = end_stream_timestamp
                    && let Some(pts) = decoded_audio_frame.pts()
                    && pts > end_timestamp
                {
                    return Ok(());
                }

                resample_encode_write(
                    resampler,
                    encoder,
                    decoded_audio_frame,
                    resampled_frame,
                    encoded_packet,
                    samples_written,
                    encoder_time_base,
                    writer,
                )?;
            }
        }

        Ok(())
    }

    /// Extract the complete audio track asynchronously.
    ///
    /// Returns an [`AudioFuture`] that
    /// resolves to the encoded audio bytes. The actual transcoding runs on
    /// a blocking thread so the async runtime is not starved.
    ///
    /// A fresh demuxer is opened internally; the mutable borrow on the
    /// unbundler is released as soon as this method returns.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::NoAudioStream`] if the file has no audio
    /// stream (validated eagerly before spawning the background thread).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use unbundle::{AudioFormat, ExtractOptions, MediaFile, UnbundleError};
    ///
    /// # async fn example() -> Result<(), UnbundleError> {
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let config = ExtractOptions::new();
    /// let audio_bytes = unbundler
    ///     .audio()
    ///     .extract_async(AudioFormat::Wav, config)?
    ///     .await?;
    /// println!("Got {} bytes of audio", audio_bytes.len());
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "async")]
    pub fn extract_async(
        &mut self,
        format: AudioFormat,
        config: ExtractOptions,
    ) -> Result<AudioFuture, UnbundleError> {
        let _stream_index = self.resolve_stream_index()?;
        let track_index = self.stream_index.and_then(|si| {
            self.unbundler
                .audio_stream_indices
                .iter()
                .position(|&idx| idx == si)
        });
        let file_path = self.unbundler.file_path.clone();
        Ok(crate::stream::create_audio_future(
            file_path,
            format,
            track_index,
            None,
            config,
        ))
    }

    /// Extract an audio time range asynchronously.
    ///
    /// Like [`extract_async`](AudioHandle::extract_async) but extracts only
    /// the segment between `start` and `end`.
    ///
    /// # Errors
    ///
    /// Returns [`UnbundleError::NoAudioStream`] if no audio stream exists, or
    /// [`UnbundleError::InvalidRange`] if `start >= end`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use unbundle::{AudioFormat, ExtractOptions, MediaFile, UnbundleError};
    ///
    /// # async fn example() -> Result<(), UnbundleError> {
    /// let mut unbundler = MediaFile::open("input.mp4")?;
    /// let audio = unbundler
    ///     .audio()
    ///     .extract_range_async(
    ///         AudioFormat::Mp3,
    ///         Duration::from_secs(10),
    ///         Duration::from_secs(20),
    ///         ExtractOptions::new(),
    ///     )?
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "async")]
    pub fn extract_range_async(
        &mut self,
        format: AudioFormat,
        start: Duration,
        end: Duration,
        config: ExtractOptions,
    ) -> Result<AudioFuture, UnbundleError> {
        let _stream_index = self.resolve_stream_index()?;
        if start >= end {
            return Err(UnbundleError::InvalidRange {
                start: format!("{start:?}"),
                end: format!("{end:?}"),
            });
        }
        let track_index = self.stream_index.and_then(|si| {
            self.unbundler
                .audio_stream_indices
                .iter()
                .position(|&idx| idx == si)
        });
        let file_path = self.unbundler.file_path.clone();
        Ok(crate::stream::create_audio_future(
            file_path,
            format,
            track_index,
            Some((start, end)),
            config,
        ))
    }
}

/// Trait abstracting how encoded audio packets are written to an output.
///
/// Two implementations exist:
/// - [`MemoryPacketWriter`]: writes to an in-memory FFmpeg dynamic buffer
/// - [`FilePacketWriter`]: writes to a file-backed FFmpeg output context
trait PacketWriter {
    /// Write a single encoded packet to the output.
    fn write_packet(&mut self, packet: &mut Packet) -> Result<(), UnbundleError>;
}

/// Writes encoded audio packets to an in-memory FFmpeg dynamic buffer.
///
/// The raw pointer is not owned; callers are responsible for lifetime and
/// cleanup of the underlying `AVFormatContext`.
struct MemoryPacketWriter {
    format_context: *mut AVFormatContext,
}

impl PacketWriter for MemoryPacketWriter {
    fn write_packet(&mut self, packet: &mut Packet) -> Result<(), UnbundleError> {
        unsafe {
            ffmpeg_sys_next::av_interleaved_write_frame(
                self.format_context,
                packet.as_mut_ptr(),
            );
        }
        Ok(())
    }
}

/// Writes encoded audio packets to a file-backed FFmpeg output context.
struct FilePacketWriter<'a> {
    output_context: &'a mut Output,
}

impl PacketWriter for FilePacketWriter<'_> {
    fn write_packet(&mut self, packet: &mut Packet) -> Result<(), UnbundleError> {
        packet
            .write_interleaved(self.output_context)
            .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))
    }
}

/// Resample a decoded frame, encode it, and write packets to the output.
#[allow(clippy::too_many_arguments)]
fn resample_encode_write<W: PacketWriter>(
    resampler: &mut ResamplingContext,
    encoder: &mut AudioEncoder,
    decoded_frame: &AudioFrame,
    resampled_frame: &mut AudioFrame,
    encoded_packet: &mut Packet,
    samples_written: &mut i64,
    encoder_time_base: Rational,
    writer: &mut W,
) -> Result<(), UnbundleError> {
    let _delay = resampler
        .run(decoded_frame, resampled_frame)
        .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;

    resampled_frame.set_pts(Some(*samples_written));
    *samples_written += resampled_frame.samples() as i64;

    encoder
        .send_frame(resampled_frame)
        .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;

    while encoder.receive_packet(encoded_packet).is_ok() {
        encoded_packet.set_stream(0);
        encoded_packet.rescale_ts(encoder_time_base, encoder_time_base);
        writer.write_packet(encoded_packet)?;
    }

    Ok(())
}
