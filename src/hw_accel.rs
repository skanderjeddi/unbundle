//! Hardware-accelerated video decoding.
//!
//! When the `hw-accel` feature is enabled, this module provides
//! [`HwAccelMode`] for controlling hardware decoder selection and internal
//! helpers that set up an FFmpeg hardware device context, configure the
//! decoder, and transfer decoded frames from GPU to system memory.
//!
//! The public entry point is [`HwAccelMode`], which is threaded through
//! [`ExtractionConfig`](crate::ExtractionConfig) via
//! [`with_hw_accel`](crate::ExtractionConfig::with_hw_accel).
//!
//! # Platform Support
//!
//! Hardware acceleration availability depends on both the FFmpeg build and
//! the host system's GPU drivers. When auto-detection fails, the decoder
//! silently falls back to software decoding.

use ffmpeg_next::{
    codec::context::Context as CodecContext,
    decoder::Video as VideoDecoder,
    frame::Video as VideoFrame,
};
use ffmpeg_sys_next::{
    AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX,
    AVBufferRef, AVCodecContext, AVCodecHWConfig, AVHWDeviceType,
};

use crate::error::UnbundleError;

/// Hardware acceleration mode for video decoding.
///
/// Used with [`ExtractionConfig::with_hw_accel`](crate::ExtractionConfig::with_hw_accel)
/// to control whether and how hardware decoding is attempted.
///
/// # Example
///
/// ```no_run
/// use unbundle::{ExtractionConfig, FrameRange, MediaUnbundler};
/// #[cfg(feature = "hw-accel")]
/// use unbundle::HwAccelMode;
///
/// let config = ExtractionConfig::new();
/// #[cfg(feature = "hw-accel")]
/// let config = config.with_hw_accel(HwAccelMode::Auto);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HwAccelMode {
    /// Automatically detect the best available hardware decoder.
    /// Falls back to software decoding if no hardware is available.
    #[default]
    Auto,
    /// Force software decoding — no hardware acceleration.
    Software,
    /// Use a specific hardware device type. Falls back to software
    /// if the requested device is not available.
    Specific(HwDeviceType),
}

/// Supported hardware device types for accelerated decoding.
///
/// Not all types are available on all platforms. Use [`HwAccelMode::Auto`]
/// to let the library choose the best available device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwDeviceType {
    /// NVIDIA CUDA (Linux, Windows).
    Cuda,
    /// Video Acceleration API (Linux).
    Vaapi,
    /// DirectX Video Acceleration 2 (Windows).
    Dxva2,
    /// Direct3D 11 Video Acceleration (Windows).
    D3d11va,
    /// Apple VideoToolbox (macOS, iOS).
    VideoToolbox,
    /// Intel Quick Sync Video (cross-platform).
    Qsv,
}

impl HwDeviceType {
    /// Convert to the FFmpeg `AVHWDeviceType` constant.
    fn to_av_hw_device_type(self) -> AVHWDeviceType {
        match self {
            HwDeviceType::Cuda => AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
            HwDeviceType::Vaapi => AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            HwDeviceType::Dxva2 => AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2,
            HwDeviceType::D3d11va => AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
            HwDeviceType::VideoToolbox => {
                AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX
            }
            HwDeviceType::Qsv => AVHWDeviceType::AV_HWDEVICE_TYPE_QSV,
        }
    }
}

/// List all hardware device types supported by the FFmpeg build.
pub fn available_hw_devices() -> Vec<HwDeviceType> {
    let mut devices = Vec::new();
    let mut device_type = AVHWDeviceType::AV_HWDEVICE_TYPE_NONE;

    loop {
        device_type = unsafe { ffmpeg_sys_next::av_hwdevice_iterate_types(device_type) };
        if device_type == AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            break;
        }

        let mapped = match device_type {
            AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA => Some(HwDeviceType::Cuda),
            AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI => Some(HwDeviceType::Vaapi),
            AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2 => Some(HwDeviceType::Dxva2),
            AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA => {
                Some(HwDeviceType::D3d11va)
            }
            AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX => {
                Some(HwDeviceType::VideoToolbox)
            }
            AVHWDeviceType::AV_HWDEVICE_TYPE_QSV => Some(HwDeviceType::Qsv),
            _ => None,
        };

        if let Some(dev) = mapped {
            devices.push(dev);
        }
    }

    devices
}

/// Outcome of attempting to set up a hardware-accelerated decoder.
pub(crate) struct HwDecoderSetup {
    /// The configured video decoder (may be HW-accelerated or software).
    pub decoder: VideoDecoder,
    /// Whether hardware acceleration was successfully enabled.
    pub hw_active: bool,
}

/// Attempt to create a hardware-accelerated decoder for the given codec
/// context.
///
/// On success, returns a decoder with an HW device context attached.
/// On failure, returns the original software decoder.
pub(crate) fn try_create_hw_decoder(
    codec_context: CodecContext,
    mode: HwAccelMode,
) -> Result<HwDecoderSetup, UnbundleError> {
    if mode == HwAccelMode::Software {
        let decoder = codec_context.decoder().video()?;
        return Ok(HwDecoderSetup {
            decoder,
            hw_active: false,
        });
    }

    let device_type = match mode {
        HwAccelMode::Auto => find_best_hw_device_for_codec(&codec_context),
        HwAccelMode::Specific(dev) => {
            let av_type = dev.to_av_hw_device_type();
            if codec_supports_hw_type(&codec_context, av_type) {
                Some(av_type)
            } else {
                None
            }
        }
        HwAccelMode::Software => unreachable!(),
    };

    let Some(av_device_type) = device_type else {
        // No suitable HW device found — fall back to software.
        let decoder = codec_context.decoder().video()?;
        return Ok(HwDecoderSetup {
            decoder,
            hw_active: false,
        });
    };

    // Try to create the HW device context.
    match create_hw_device_context(av_device_type) {
        Ok(hw_device_ctx) => {
            // Attach to the codec context and create the decoder.
            unsafe {
                let ctx_ptr = codec_context.as_ptr() as *mut AVCodecContext;
                (*ctx_ptr).hw_device_ctx =
                    ffmpeg_sys_next::av_buffer_ref(hw_device_ctx);
            }
            let decoder = codec_context.decoder().video()?;

            // Clean up our reference (the decoder now holds its own ref).
            unsafe {
                let mut hw_ref = hw_device_ctx;
                ffmpeg_sys_next::av_buffer_unref(&mut hw_ref);
            }

            Ok(HwDecoderSetup {
                decoder,
                hw_active: true,
            })
        }
        Err(_) => {
            // HW device creation failed — fall back to software.
            let decoder = codec_context.decoder().video()?;
            Ok(HwDecoderSetup {
                decoder,
                hw_active: false,
            })
        }
    }
}

/// Transfer a hardware frame to system memory.
///
/// If the frame is already in system memory, it is returned as-is.
/// Otherwise, allocates a new software frame and copies the data.
pub(crate) fn transfer_hw_frame(hw_frame: &VideoFrame) -> Result<VideoFrame, UnbundleError> {
    let format = unsafe { (*hw_frame.as_ptr()).format };

    // Check if it's a "hardware" pixel format by seeing if data[0] is null
    // or the format indicates a HW surface. A pragmatic check: if the
    // frame's data pointer is populated and format > 0, try transfer anyway.
    // `av_hwframe_transfer_data` will return an error if it's not an HW frame.
    let mut sw_frame = VideoFrame::empty();

    let ret = unsafe {
        ffmpeg_sys_next::av_hwframe_transfer_data(sw_frame.as_mut_ptr(), hw_frame.as_ptr(), 0)
    };

    if ret < 0 {
        // Not an HW frame or transfer failed. If the format is a normal
        // pixel format, the caller should just use the original frame.
        // Return an error so the caller can fall back.
        Err(UnbundleError::VideoDecodeError(format!(
            "HW frame transfer failed (format={format}, ret={ret})"
        )))
    } else {
        // Copy PTS and other timing info.
        unsafe {
            (*sw_frame.as_mut_ptr()).pts = (*hw_frame.as_ptr()).pts;
            (*sw_frame.as_mut_ptr()).pkt_dts = (*hw_frame.as_ptr()).pkt_dts;
        }
        Ok(sw_frame)
    }
}

/// Find the best hardware device type supported by the codec.
fn find_best_hw_device_for_codec(codec_context: &CodecContext) -> Option<AVHWDeviceType> {
    let codec_ptr = unsafe { (*codec_context.as_ptr()).codec };
    if codec_ptr.is_null() {
        return None;
    }

    let mut index: i32 = 0;
    let mut best: Option<AVHWDeviceType> = None;

    loop {
        let config: *const AVCodecHWConfig =
            unsafe { ffmpeg_sys_next::avcodec_get_hw_config(codec_ptr, index) };
        if config.is_null() {
            break;
        }

        let methods = unsafe { (*config).methods };
        if methods & (AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32) != 0 {
            let device_type = unsafe { (*config).device_type };
            if device_type != AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
                // Prefer the first supported device.
                if best.is_none() {
                    best = Some(device_type);
                }
            }
        }

        index += 1;
    }

    best
}

/// Check whether a codec supports a specific HW device type.
fn codec_supports_hw_type(
    codec_context: &CodecContext,
    device_type: AVHWDeviceType,
) -> bool {
    let codec_ptr = unsafe { (*codec_context.as_ptr()).codec };
    if codec_ptr.is_null() {
        return false;
    }

    let mut index: i32 = 0;

    loop {
        let config: *const AVCodecHWConfig =
            unsafe { ffmpeg_sys_next::avcodec_get_hw_config(codec_ptr, index) };
        if config.is_null() {
            break;
        }

        let methods = unsafe { (*config).methods };
        let dt = unsafe { (*config).device_type };
        if methods & (AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32) != 0
            && dt == device_type
        {
            return true;
        }

        index += 1;
    }

    false
}

/// Create an FFmpeg HW device context.
///
/// Returns a raw `AVBufferRef*` that must be freed with `av_buffer_unref`.
fn create_hw_device_context(
    device_type: AVHWDeviceType,
) -> Result<*mut AVBufferRef, UnbundleError> {
    let mut hw_device_ctx: *mut AVBufferRef = std::ptr::null_mut();

    let ret = unsafe {
        ffmpeg_sys_next::av_hwdevice_ctx_create(
            &mut hw_device_ctx,
            device_type,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
        )
    };

    if ret < 0 {
        Err(UnbundleError::VideoDecodeError(format!(
            "Failed to create HW device context (ret={ret})"
        )))
    } else {
        Ok(hw_device_ctx)
    }
}
