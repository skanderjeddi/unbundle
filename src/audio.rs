//! Audio extraction.
//!
//! This module provides [`AudioExtractor`] for extracting audio tracks from
//! media files, and [`AudioFormat`] for specifying the output encoding.
//! Audio can be extracted to memory as `Vec<u8>` or written directly to a file.

use std::{ffi::CString, path::Path, ptr, time::Duration};

use ffmpeg_next::{
    ChannelLayout, Packet, Rational,
    codec::{Id, context::Context as CodecContext},
    decoder::Audio as AudioDecoder,
    encoder::Audio as AudioEncoder,
    format::{Sample, context::Output, sample::Type as SampleType},
    frame::Audio as AudioFrame,
    packet::Mut as PacketMut,
    software::resampling::Context as ResamplingContext,
};
use ffmpeg_sys_next::{AVFormatContext, AVRational};

use crate::{error::UnbundleError, unbundler::MediaUnbundler};

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
/// Obtained via [`MediaUnbundler::audio`]. Provides methods for extracting
/// complete audio tracks or segments, either to memory or to files.
pub struct AudioExtractor<'a> {
    pub(crate) unbundler: &'a mut MediaUnbundler,
}

impl<'a> AudioExtractor<'a> {
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
    /// use unbundle::{AudioFormat, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let audio_bytes = unbundler.audio().extract(AudioFormat::Wav)?;
    /// println!("Extracted {} bytes", audio_bytes.len());
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn extract(&mut self, format: AudioFormat) -> Result<Vec<u8>, UnbundleError> {
        self.extract_audio_to_memory(format, None, None)
    }

    /// Extract an audio segment by time range to memory.
    ///
    /// Extracts audio between `start` and `end` timestamps (inclusive).
    ///
    /// # Errors
    ///
    /// Returns errors from [`extract`](AudioExtractor::extract), plus
    /// [`UnbundleError::InvalidTimestamp`] if either timestamp exceeds the
    /// media duration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use unbundle::{AudioFormat, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// let segment = unbundler.audio().extract_range(
    ///     Duration::from_secs(10),
    ///     Duration::from_secs(20),
    ///     AudioFormat::Mp3,
    /// )?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn extract_range(
        &mut self,
        start: Duration,
        end: Duration,
        format: AudioFormat,
    ) -> Result<Vec<u8>, UnbundleError> {
        self.extract_audio_to_memory(format, Some(start), Some(end))
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
    /// use unbundle::{AudioFormat, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// unbundler.audio().save("output.wav", AudioFormat::Wav)?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn save<P: AsRef<Path>>(
        &mut self,
        path: P,
        format: AudioFormat,
    ) -> Result<(), UnbundleError> {
        self.save_audio_to_file(path.as_ref(), format, None, None)
    }

    /// Save an audio segment to a file.
    ///
    /// # Errors
    ///
    /// Returns errors from [`save`](AudioExtractor::save), plus
    /// [`UnbundleError::InvalidTimestamp`] if either timestamp exceeds the
    /// media duration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use unbundle::{AudioFormat, MediaUnbundler};
    ///
    /// let mut unbundler = MediaUnbundler::open("input.mp4")?;
    /// unbundler.audio().save_range(
    ///     "segment.mp3",
    ///     Duration::from_secs(30),
    ///     Duration::from_secs(60),
    ///     AudioFormat::Mp3,
    /// )?;
    /// # Ok::<(), unbundle::UnbundleError>(())
    /// ```
    pub fn save_range<P: AsRef<Path>>(
        &mut self,
        path: P,
        start: Duration,
        end: Duration,
        format: AudioFormat,
    ) -> Result<(), UnbundleError> {
        self.save_audio_to_file(path.as_ref(), format, Some(start), Some(end))
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
    ) -> Result<Vec<u8>, UnbundleError> {
        let audio_stream_index = self
            .unbundler
            .audio_stream_index
            .ok_or(UnbundleError::NoAudioStream)?;

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
                crate::utilities::duration_to_stream_timestamp(start_time, input_time_base);
            self.unbundler
                .input_context
                .seek(start_timestamp, ..start_timestamp)?;
        }

        // Compute end timestamp in stream time base for range filtering.
        let end_stream_timestamp = end.map(|end_time| {
            crate::utilities::duration_to_stream_timestamp(end_time, input_time_base)
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

            let mut output_format_context: *mut AVFormatContext = ptr::null_mut();
            let allocation_result = ffmpeg_sys_next::avformat_alloc_output_context2(
                &mut output_format_context,
                ptr::null_mut(),
                container_name_c.as_ptr(),
                ptr::null(),
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
                ffmpeg_sys_next::avformat_new_stream(output_format_context, ptr::null());
            if output_stream.is_null() {
                let mut buffer_pointer: *mut u8 = ptr::null_mut();
                ffmpeg_sys_next::avio_close_dyn_buf(
                    (*output_format_context).pb,
                    &mut buffer_pointer,
                );
                if !buffer_pointer.is_null() {
                    ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                }
                (*output_format_context).pb = ptr::null_mut();
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
                    let mut buffer_pointer: *mut u8 = ptr::null_mut();
                    ffmpeg_sys_next::avio_close_dyn_buf(
                        (*output_format_context).pb,
                        &mut buffer_pointer,
                    );
                    if !buffer_pointer.is_null() {
                        ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                    }
                    (*output_format_context).pb = ptr::null_mut();
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
                ffmpeg_sys_next::avformat_write_header(output_format_context, ptr::null_mut());
            if write_header_result < 0 {
                let mut buffer_pointer: *mut u8 = ptr::null_mut();
                ffmpeg_sys_next::avio_close_dyn_buf(
                    (*output_format_context).pb,
                    &mut buffer_pointer,
                );
                if !buffer_pointer.is_null() {
                    ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                }
                (*output_format_context).pb = ptr::null_mut();
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

            let transcode_result = self.transcode_audio_packets(
                audio_stream_index,
                &mut decoder,
                &mut resampler,
                &mut encoder,
                &mut decoded_audio_frame,
                &mut resampled_frame,
                &mut encoded_packet,
                &mut samples_written,
                input_time_base,
                encoder_time_base,
                end_stream_timestamp,
                output_format_context,
            );

            if let Err(error) = transcode_result {
                let mut buffer_pointer: *mut u8 = ptr::null_mut();
                ffmpeg_sys_next::avio_close_dyn_buf(
                    (*output_format_context).pb,
                    &mut buffer_pointer,
                );
                if !buffer_pointer.is_null() {
                    ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                }
                (*output_format_context).pb = ptr::null_mut();
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
                    output_format_context,
                ) {
                    let mut buffer_pointer: *mut u8 = ptr::null_mut();
                    ffmpeg_sys_next::avio_close_dyn_buf(
                        (*output_format_context).pb,
                        &mut buffer_pointer,
                    );
                    if !buffer_pointer.is_null() {
                        ffmpeg_sys_next::av_free(buffer_pointer as *mut _);
                    }
                    (*output_format_context).pb = ptr::null_mut();
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
            let mut buffer_pointer: *mut u8 = ptr::null_mut();
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
            (*output_format_context).pb = ptr::null_mut();
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
    ) -> Result<(), UnbundleError> {
        let audio_stream_index = self
            .unbundler
            .audio_stream_index
            .ok_or(UnbundleError::NoAudioStream)?;

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
                crate::utilities::duration_to_stream_timestamp(start_time, input_time_base);
            self.unbundler
                .input_context
                .seek(start_timestamp, ..start_timestamp)?;
        }

        let end_stream_timestamp = end.map(|end_time| {
            crate::utilities::duration_to_stream_timestamp(end_time, input_time_base)
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

        // Decode → resample → encode → write loop.
        self.transcode_audio_packets_to_file(
            audio_stream_index,
            &mut decoder,
            &mut resampler,
            &mut encoder,
            &mut decoded_audio_frame,
            &mut resampled_frame,
            &mut encoded_packet,
            &mut samples_written,
            input_time_base,
            encoder_time_base,
            end_stream_timestamp,
            &mut output_context,
        )?;

        // Flush decoder.
        let _ = decoder.send_eof();
        while decoder.receive_frame(&mut decoded_audio_frame).is_ok() {
            resample_encode_write_to_file(
                &mut resampler,
                &mut encoder,
                &decoded_audio_frame,
                &mut resampled_frame,
                &mut encoded_packet,
                &mut samples_written,
                encoder_time_base,
                &mut output_context,
            )?;
        }

        // Flush encoder.
        let _ = encoder.send_eof();
        while encoder.receive_packet(&mut encoded_packet).is_ok() {
            encoded_packet.set_stream(0);
            encoded_packet.rescale_ts(encoder_time_base, encoder_time_base);
            encoded_packet
                .write_interleaved(&mut output_context)
                .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;
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

    /// Decode, resample, encode, and write audio packets (in-memory variant).
    ///
    /// # Safety
    ///
    /// `output_format_context` must be a valid, non-null pointer to an
    /// `AVFormatContext` with an open dynamic buffer attached to `pb`.
    #[allow(clippy::too_many_arguments)]
    unsafe fn transcode_audio_packets(
        &mut self,
        audio_stream_index: usize,
        decoder: &mut AudioDecoder,
        resampler: &mut ResamplingContext,
        encoder: &mut AudioEncoder,
        decoded_audio_frame: &mut AudioFrame,
        resampled_frame: &mut AudioFrame,
        encoded_packet: &mut Packet,
        samples_written: &mut i64,
        _input_time_base: Rational,
        encoder_time_base: Rational,
        end_stream_timestamp: Option<i64>,
        output_format_context: *mut AVFormatContext,
    ) -> Result<(), UnbundleError> {
        for (stream, packet) in self.unbundler.input_context.packets() {
            if stream.index() != audio_stream_index {
                continue;
            }

            // Check if we've passed the end timestamp.
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
                // Check frame timestamp against end bound.
                if let Some(end_timestamp) = end_stream_timestamp
                    && let Some(pts) = decoded_audio_frame.pts()
                    && pts > end_timestamp
                {
                    return Ok(());
                }

                unsafe {
                    resample_encode_write(
                        resampler,
                        encoder,
                        decoded_audio_frame,
                        resampled_frame,
                        encoded_packet,
                        samples_written,
                        encoder_time_base,
                        output_format_context,
                    )
                }?;
            }
        }

        Ok(())
    }

    /// Decode, resample, encode, and write audio packets (file variant).
    #[allow(clippy::too_many_arguments)]
    fn transcode_audio_packets_to_file(
        &mut self,
        audio_stream_index: usize,
        decoder: &mut AudioDecoder,
        resampler: &mut ResamplingContext,
        encoder: &mut AudioEncoder,
        decoded_audio_frame: &mut AudioFrame,
        resampled_frame: &mut AudioFrame,
        encoded_packet: &mut Packet,
        samples_written: &mut i64,
        _input_time_base: Rational,
        encoder_time_base: Rational,
        end_stream_timestamp: Option<i64>,
        output_context: &mut Output,
    ) -> Result<(), UnbundleError> {
        for (stream, packet) in self.unbundler.input_context.packets() {
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

                resample_encode_write_to_file(
                    resampler,
                    encoder,
                    decoded_audio_frame,
                    resampled_frame,
                    encoded_packet,
                    samples_written,
                    encoder_time_base,
                    output_context,
                )?;
            }
        }

        Ok(())
    }
}

/// Resample a decoded frame, encode it, and write packets to the in-memory
/// output context.
///
/// # Safety
///
/// `output_format_context` must be a valid pointer to an `AVFormatContext`
/// with an open I/O context.
#[allow(clippy::too_many_arguments)]
unsafe fn resample_encode_write(
    resampler: &mut ResamplingContext,
    encoder: &mut AudioEncoder,
    decoded_frame: &AudioFrame,
    resampled_frame: &mut AudioFrame,
    encoded_packet: &mut Packet,
    samples_written: &mut i64,
    encoder_time_base: Rational,
    output_format_context: *mut AVFormatContext,
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
        unsafe {
            ffmpeg_sys_next::av_interleaved_write_frame(
                output_format_context,
                encoded_packet.as_mut_ptr(),
            );
        }
    }

    Ok(())
}

/// Resample, encode, and write packets to a file-backed output context.
#[allow(clippy::too_many_arguments)]
fn resample_encode_write_to_file(
    resampler: &mut ResamplingContext,
    encoder: &mut AudioEncoder,
    decoded_frame: &AudioFrame,
    resampled_frame: &mut AudioFrame,
    encoded_packet: &mut Packet,
    samples_written: &mut i64,
    encoder_time_base: Rational,
    output_context: &mut Output,
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
        encoded_packet
            .write_interleaved(output_context)
            .map_err(|error| UnbundleError::AudioEncodeError(error.to_string()))?;
    }

    Ok(())
}
