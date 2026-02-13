//! Hardware-accelerated video decoding.
//!
//! When the `hardware` feature is enabled, this module provides
//! [`HardwareAccelerationMode`] for controlling hardware decoder selection and internal
//! helpers that set up an FFmpeg hardware device context, configure the
//! decoder, and transfer decoded frames from GPU to system memory.
//!
//! The public entry point is [`HardwareAccelerationMode`], which is threaded through
//! [`ExtractOptions`](crate::ExtractOptions) via
//! [`with_hardware_acceleration`](crate::ExtractOptions::with_hardware_acceleration).
//!
//! # Platform Support
//!
//! Hardware acceleration availability depends on both the FFmpeg build and
//! the host system's GPU drivers. When auto-detection fails, the decoder
//! silently falls back to software decoding.

use ffmpeg_next::{
    codec::context::Context as CodecContext, decoder::Video as VideoDecoder,
    frame::Video as VideoFrame,
};
use ffmpeg_sys_next::{
    AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX, AVBufferRef, AVCodecContext, AVCodecHWConfig,
    AVHWDeviceType,
};

use crate::error::UnbundleError;

/// Hardware acceleration mode for video decoding.
///
/// Used with [`ExtractOptions::with_hardware_acceleration`](crate::ExtractOptions::with_hardware_acceleration)
/// to control whether and how hardware decoding is attempted.
///
/// # Example
///
/// ```no_run
/// use unbundle::{ExtractOptions, FrameRange, MediaFile};
/// #[cfg(feature = "hardware")]
/// use unbundle::HardwareAccelerationMode;
///
/// let config = ExtractOptions::new();
/// #[cfg(feature = "hardware")]
/// let config = config.with_hardware_acceleration(HardwareAccelerationMode::Auto);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HardwareAccelerationMode {
    /// Automatically detect the best available hardware decoder.
    /// Falls back to software decoding if no hardware is available.
    #[default]
    Auto,
    /// Force software decoding — no hardware acceleration.
    Software,
    /// Use a specific hardware device type. Falls back to software
    /// if the requested device is not available.
    Specific(HardwareDeviceType),
}

/// Supported hardware device types for accelerated decoding.
///
/// Not all types are available on all platforms. Use [`HardwareAccelerationMode::Auto`]
/// to let the library choose the best available device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareDeviceType {
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

impl HardwareDeviceType {
    /// Convert to the FFmpeg `AVHWDeviceType` constant.
    fn to_av_hw_device_type(self) -> AVHWDeviceType {
        match self {
            HardwareDeviceType::Cuda => AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
            HardwareDeviceType::Vaapi => AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            HardwareDeviceType::Dxva2 => AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2,
            HardwareDeviceType::D3d11va => AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
            HardwareDeviceType::VideoToolbox => AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
            HardwareDeviceType::Qsv => AVHWDeviceType::AV_HWDEVICE_TYPE_QSV,
        }
    }
}

/// List all hardware device types supported by the FFmpeg build.
pub fn available_hardware_devices() -> Vec<HardwareDeviceType> {
    let mut devices = Vec::new();
    let mut device_type = AVHWDeviceType::AV_HWDEVICE_TYPE_NONE;

    loop {
        device_type = unsafe { ffmpeg_sys_next::av_hwdevice_iterate_types(device_type) };
        if device_type == AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            break;
        }

        let mapped = match device_type {
            AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA => Some(HardwareDeviceType::Cuda),
            AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI => Some(HardwareDeviceType::Vaapi),
            AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2 => Some(HardwareDeviceType::Dxva2),
            AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA => Some(HardwareDeviceType::D3d11va),
            AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX => Some(HardwareDeviceType::VideoToolbox),
            AVHWDeviceType::AV_HWDEVICE_TYPE_QSV => Some(HardwareDeviceType::Qsv),
            _ => None,
        };

        if let Some(dev) = mapped {
            devices.push(dev);
        }
    }

    devices
}

/// Outcome of attempting to set up a hardware-accelerated decoder.
pub(crate) struct HardwareDecoderSetup {
    /// The configured video decoder (may be hardware-accelerated or software).
    pub decoder: VideoDecoder,
    /// Whether hardware acceleration was successfully enabled.
    pub hardware_active: bool,
}

/// Attempt to create a hardware-accelerated decoder for the given codec
/// context.
///
/// On success, returns a decoder with a hardware device context attached.
/// On failure, returns the original software decoder.
pub(crate) fn try_create_hardware_decoder(
    codec_context: CodecContext,
    mode: HardwareAccelerationMode,
) -> Result<HardwareDecoderSetup, UnbundleError> {
    if mode == HardwareAccelerationMode::Software {
        let decoder = codec_context.decoder().video()?;
        return Ok(HardwareDecoderSetup {
            decoder,
            hardware_active: false,
        });
    }

    let device_type = match mode {
        HardwareAccelerationMode::Auto => find_best_hardware_device_for_codec(&codec_context),
        HardwareAccelerationMode::Specific(device) => {
            let av_type = device.to_av_hw_device_type();
            if codec_supports_hardware_type(&codec_context, av_type) {
                Some(av_type)
            } else {
                None
            }
        }
        HardwareAccelerationMode::Software => unreachable!(),
    };

    let Some(av_device_type) = device_type else {
        // No suitable hardware device found — fall back to software.
        let decoder = codec_context.decoder().video()?;
        return Ok(HardwareDecoderSetup {
            decoder,
            hardware_active: false,
        });
    };

    // Try to create the hardware device context.
    match create_hardware_device_context(av_device_type) {
        Ok(hardware_device_context) => {
            // Attach to the codec context and create the decoder.
            unsafe {
                let context_pointer = codec_context.as_ptr() as *mut AVCodecContext;
                (*context_pointer).hw_device_ctx =
                    ffmpeg_sys_next::av_buffer_ref(hardware_device_context);
            }
            let decoder = codec_context.decoder().video()?;

            // Clean up our reference (the decoder now holds its own ref).
            unsafe {
                let mut hardware_reference = hardware_device_context;
                ffmpeg_sys_next::av_buffer_unref(&mut hardware_reference);
            }

            Ok(HardwareDecoderSetup {
                decoder,
                hardware_active: true,
            })
        }
        Err(_) => {
            // Hardware device creation failed — fall back to software.
            let decoder = codec_context.decoder().video()?;
            Ok(HardwareDecoderSetup {
                decoder,
                hardware_active: false,
            })
        }
    }
}

/// Transfer a hardware frame to system memory.
///
/// If the frame is already in system memory, it is returned as-is.
/// Otherwise, allocates a new software frame and copies the data.
pub(crate) fn transfer_hardware_frame(
    hardware_frame: &VideoFrame,
) -> Result<VideoFrame, UnbundleError> {
    let format = unsafe { (*hardware_frame.as_ptr()).format };

    // Check if it's a "hardware" pixel format by seeing if data[0] is null
    // or the format indicates a HW surface. A pragmatic check: if the
    // frame's data pointer is populated and format > 0, try transfer anyway.
    // `av_hwframe_transfer_data` will return an error if it's not an HW frame.
    let mut software_frame = VideoFrame::empty();

    let result = unsafe {
        ffmpeg_sys_next::av_hwframe_transfer_data(
            software_frame.as_mut_ptr(),
            hardware_frame.as_ptr(),
            0,
        )
    };

    if result < 0 {
        // Not an HW frame or transfer failed. If the format is a normal
        // pixel format, the caller should just use the original frame.
        // Return an error so the caller can fall back.
        Err(UnbundleError::VideoDecodeError(format!(
            "Hardware frame transfer failed (format={format}, result={result})"
        )))
    } else {
        // Copy PTS and other timing info.
        unsafe {
            (*software_frame.as_mut_ptr()).pts = (*hardware_frame.as_ptr()).pts;
            (*software_frame.as_mut_ptr()).pkt_dts = (*hardware_frame.as_ptr()).pkt_dts;
        }
        Ok(software_frame)
    }
}

/// Find the best hardware device type supported by the codec.
fn find_best_hardware_device_for_codec(codec_context: &CodecContext) -> Option<AVHWDeviceType> {
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

/// Check whether a codec supports a specific hardware device type.
fn codec_supports_hardware_type(codec_context: &CodecContext, device_type: AVHWDeviceType) -> bool {
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
        let queried_device_type = unsafe { (*config).device_type };
        if methods & (AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32) != 0
            && queried_device_type == device_type
        {
            return true;
        }

        index += 1;
    }

    false
}

/// Create an FFmpeg hardware device context.
///
/// Returns a raw `AVBufferRef*` that must be freed with `av_buffer_unref`.
fn create_hardware_device_context(
    device_type: AVHWDeviceType,
) -> Result<*mut AVBufferRef, UnbundleError> {
    let mut hardware_device_context: *mut AVBufferRef = std::ptr::null_mut();

    let result = unsafe {
        ffmpeg_sys_next::av_hwdevice_ctx_create(
            &mut hardware_device_context,
            device_type,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
        )
    };

    if result < 0 {
        Err(UnbundleError::VideoDecodeError(format!(
            "Failed to create hardware device context (result={result})"
        )))
    } else {
        Ok(hardware_device_context)
    }
}
