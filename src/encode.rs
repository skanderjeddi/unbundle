//! Video encoder — encode a sequence of frames into a video file.
//!
//! This module provides [`VideoEncoder`] for encoding `DynamicImage` frames into
//! a video container (MP4, MKV, AVI, etc.) using FFmpeg.
//!
//! # Example
//!
//! ```no_run
//! use unbundle::{FrameRange, MediaFile, UnbundleError, VideoEncoder, VideoEncoderOptions};
//!
//! let mut unbundler = MediaFile::open("input.mp4")?;
//! let frames = unbundler.video().frames(FrameRange::Range(0, 10))?;
//! VideoEncoder::new(VideoEncoderOptions::default())
//!     .write("output.mp4", &frames)?;
//! # Ok::<(), UnbundleError>(())
//! ```

use std::path::Path;

use ffmpeg_next::codec::Id;
use ffmpeg_next::codec::context::Context as CodecContext;
use ffmpeg_next::format::{Flags as FormatFlags, Pixel};
use ffmpeg_next::frame::Video as VideoFrame;
use ffmpeg_next::software::scaling::{Context as ScalingContext, Flags as ScalingFlags};
use ffmpeg_next::{Packet, Rational};
use image::DynamicImage;
use image::imageops::FilterType;

use crate::error::UnbundleError;

/// Options for the video encoder.
///
/// Controls the output codec, frame rate, resolution, and quality.
#[derive(Debug, Clone)]
pub struct VideoEncoderOptions {
    /// Target frames per second (default: 30).
    pub fps: u32,
    /// Output width. If `None`, inferred from the first frame.
    pub width: Option<u32>,
    /// Output height. If `None`, inferred from the first frame.
    pub height: Option<u32>,
    /// Codec to use. Default is H.264.
    pub codec: VideoCodec,
    /// Constant Rate Factor for quality (0-51, lower is better). Default: 23.
    pub crf: Option<u32>,
    /// Bitrate in bits per second. If set, overrides CRF.
    pub bitrate: Option<usize>,
}

impl Default for VideoEncoderOptions {
    fn default() -> Self {
        Self {
            fps: 30,
            width: None,
            height: None,
            codec: VideoCodec::H264,
            crf: Some(23),
            bitrate: None,
        }
    }
}

impl VideoEncoderOptions {
    /// Set the frame rate.
    pub fn fps(mut self, fps: u32) -> Self {
        self.fps = fps;
        self
    }

    /// Set the output resolution.
    pub fn resolution(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    /// Set the codec.
    pub fn codec(mut self, codec: VideoCodec) -> Self {
        self.codec = codec;
        self
    }

    /// Set the CRF quality value.
    pub fn crf(mut self, crf: u32) -> Self {
        self.crf = Some(crf);
        self
    }

    /// Set the target bitrate in bits per second.
    pub fn bitrate(mut self, bitrate: usize) -> Self {
        self.bitrate = Some(bitrate);
        self
    }
}

/// Supported output video codecs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264 / AVC.
    H264,
    /// H.265 / HEVC.
    H265,
    /// MPEG-4 Part 2 (for AVI compatibility).
    Mpeg4,
}

impl VideoCodec {
    fn to_codec_id(self) -> Id {
        match self {
            VideoCodec::H264 => Id::H264,
            VideoCodec::H265 => Id::HEVC,
            VideoCodec::Mpeg4 => Id::MPEG4,
        }
    }

    fn input_pixel_format(self) -> Pixel {
        // H.264/H.265 encoders prefer YUV420P input; MPEG4 also works with YUV420P.
        Pixel::YUV420P
    }
}

/// Encodes a sequence of frames into a video file.
///
/// Create via [`VideoEncoder::new`], then call [`write`](VideoEncoder::write).
pub struct VideoEncoder {
    config: VideoEncoderOptions,
}

impl VideoEncoder {
    /// Create a new video encoder with the given options.
    pub fn new(config: VideoEncoderOptions) -> Self {
        Self { config }
    }

    /// Write frames to the output path.
    ///
    /// The container format is inferred from the file extension.
    ///
    /// # Errors
    ///
    /// - [`UnbundleError::VideoWriteError`] on encoding or I/O failure.
    /// - [`UnbundleError::VideoEncodeError`] if the codec cannot be opened.
    pub fn write<P: AsRef<Path>>(
        &self,
        path: P,
        frames: &[DynamicImage],
    ) -> Result<(), UnbundleError> {
        log::info!(
            "Writing {} frames to {:?} (codec={:?}, fps={})",
            frames.len(), path.as_ref(), self.config.codec, self.config.fps,
        );
        if frames.is_empty() {
            return Err(UnbundleError::VideoWriteError(
                "no frames to write".to_string(),
            ));
        }

        let path = path.as_ref();

        // Determine output resolution from config or first frame.
        let first = &frames[0];
        let width = self.config.width.unwrap_or(first.width());
        let height = self.config.height.unwrap_or(first.height());

        let codec_id = self.config.codec.to_codec_id();
        let target_pixel = self.config.codec.input_pixel_format();

        // Open the output format context.
        let mut output = ffmpeg_next::format::output(path)
            .map_err(|e| UnbundleError::VideoWriteError(format!("cannot open output: {e}")))?;

        // Check if we need global header before adding the stream (avoids borrow conflict).
        let needs_global_header = output.format().flags().contains(FormatFlags::GLOBAL_HEADER);

        // Find encoder.
        let encoder_codec = ffmpeg_next::encoder::find(codec_id)
            .ok_or_else(|| {
                UnbundleError::VideoEncodeError(format!("codec {codec_id:?} not available"))
            })?;

        // Add video stream.
        let mut stream = output.add_stream(encoder_codec)
            .map_err(|e| UnbundleError::VideoWriteError(format!("cannot add stream: {e}")))?;

        let stream_index = stream.index();

        // Configure encoder context from the stream's codec parameters.
        let mut encoder = {
            let ctx = CodecContext::from_parameters(stream.parameters())
                .map_err(|e| {
                    UnbundleError::VideoEncodeError(format!("cannot create codec context: {e}"))
                })?;
            ctx.encoder().video()
                .map_err(|e| {
                    UnbundleError::VideoEncodeError(format!("cannot open video encoder: {e}"))
                })?
        };

        encoder.set_width(width);
        encoder.set_height(height);
        encoder.set_format(target_pixel);
        encoder.set_time_base(Rational::new(1, self.config.fps as i32));
        encoder.set_frame_rate(Some(Rational::new(self.config.fps as i32, 1)));

        if let Some(bitrate) = self.config.bitrate {
            encoder.set_bit_rate(bitrate);
        }

        // Set global header flag if the format requires it.
        if needs_global_header {
            unsafe {
                (*encoder.as_mut_ptr()).flags |= ffmpeg_sys_next::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
        }

        let mut opened_encoder = encoder.open_as(encoder_codec)
            .map_err(|e| {
                UnbundleError::VideoEncodeError(format!("cannot open encoder: {e}"))
            })?;

        // Copy encoder parameters back to the stream.
        stream.set_parameters(&opened_encoder);

        // Write file header.
        output.write_header()
            .map_err(|e| UnbundleError::VideoWriteError(format!("cannot write header: {e}")))?;

        // Set up scaler from RGB24 → target pixel format.
        let mut scaler = ScalingContext::get(
            Pixel::RGB24,
            width,
            height,
            target_pixel,
            width,
            height,
            ScalingFlags::BILINEAR,
        )
        .map_err(|e| {
            UnbundleError::VideoWriteError(format!("cannot create scaler: {e}"))
        })?;

        let mut frame_index: i64 = 0;

        for img in frames {
            // Resize if needed and convert to RGB8.
            let rgb = if img.width() != width || img.height() != height {
                img.resize_exact(width, height, FilterType::Lanczos3)
                    .to_rgb8()
            } else {
                img.to_rgb8()
            };

            // Create source frame.
            let mut src_frame = VideoFrame::new(Pixel::RGB24, width, height);
            let stride = src_frame.stride(0);
            let src_data = src_frame.data_mut(0);
            let rgb_bytes = rgb.as_raw();
            for y in 0..height as usize {
                let src_start = y * (width as usize) * 3;
                let dst_start = y * stride;
                let row_len = (width as usize) * 3;
                src_data[dst_start..dst_start + row_len]
                    .copy_from_slice(&rgb_bytes[src_start..src_start + row_len]);
            }

            // Scale to target pixel format.
            let mut dst_frame = VideoFrame::empty();
            scaler.run(&src_frame, &mut dst_frame)
                .map_err(|e| {
                    UnbundleError::VideoWriteError(format!("scaling failed: {e}"))
                })?;

            dst_frame.set_pts(Some(frame_index));
            frame_index += 1;

            // Send frame to encoder.
            opened_encoder.send_frame(&dst_frame)
                .map_err(|e| {
                    UnbundleError::VideoEncodeError(format!("send_frame failed: {e}"))
                })?;

            // Receive and write encoded packets.
            let mut packet = Packet::empty();
            while opened_encoder.receive_packet(&mut packet).is_ok() {
                packet.set_stream(stream_index);
                packet.rescale_ts(
                    Rational::new(1, self.config.fps as i32),
                    output.stream(stream_index).unwrap().time_base(),
                );
                packet.write_interleaved(&mut output)
                    .map_err(|e| {
                        UnbundleError::VideoWriteError(format!("write packet failed: {e}"))
                    })?;
            }
        }

        // Flush encoder.
        opened_encoder.send_eof()
            .map_err(|e| {
                UnbundleError::VideoEncodeError(format!("send_eof failed: {e}"))
            })?;

        let mut packet = Packet::empty();
        while opened_encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(stream_index);
            packet.rescale_ts(
                Rational::new(1, self.config.fps as i32),
                output.stream(stream_index).unwrap().time_base(),
            );
            packet.write_interleaved(&mut output)
                .map_err(|e| {
                    UnbundleError::VideoWriteError(format!("write flush packet failed: {e}"))
                })?;
        }

        // Write trailer.
        output.write_trailer()
            .map_err(|e| UnbundleError::VideoWriteError(format!("cannot write trailer: {e}")))?;

        Ok(())
    }
}
