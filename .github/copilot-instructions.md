# Copilot instructions for unbundle

## Big picture architecture
- `MediaUnbundler` is the main entry point; it opens the media file, caches `MediaMetadata`, and stores stream indexes. See [src/unbundler.rs](../src/unbundler.rs).
- `VideoExtractor`, `AudioExtractor`, and `SubtitleExtractor` are lightweight, short-lived views that borrow the unbundler mutably; you cannot hold multiple extractors at the same time. See [src/video.rs](../src/video.rs), [src/audio.rs](../src/audio.rs), and [src/subtitle.rs](../src/subtitle.rs).
- Video decoding always creates a fresh decoder per call, seeks to a keyframe, then decodes forward. Frame selection is centralized in `FrameRange`, with range/interval/time-based variants. See [src/video.rs](../src/video.rs).
- Audio extraction can target files or memory; in-memory output uses FFmpeg dynamic buffer I/O via `ffmpeg_sys_next`. See [src/audio.rs](../src/audio.rs).
- Subtitle extraction decodes text-based subtitle tracks and can export to SRT, WebVTT, or raw text. See [src/subtitle.rs](../src/subtitle.rs).
- Shared timestamp and pixel-buffer helpers live in [src/utilities.rs](../src/utilities.rs); most conversions go through these helpers rather than inline math.
- All fallible operations return `UnbundleError`; error variants carry context like file paths, frame numbers, and timestamps. The error enum is `#[non_exhaustive]`. See [src/error.rs](../src/error.rs).
- `ExtractionConfig` threads progress callbacks, cancellation tokens, pixel format, and resolution settings through extraction methods. See [src/config.rs](../src/config.rs).
- `ProgressCallback` (infallible, `Send + Sync`) and `CancellationToken` (`Arc<AtomicBool>`) provide cooperative progress/cancellation. See [src/progress.rs](../src/progress.rs).
- `FrameIterator` provides lazy, pull-based frame iteration using `Packet::read` for packet-level control. See [src/iterator.rs](../src/iterator.rs).
- `Remuxer` performs lossless container format conversion without re-encoding. See [src/convert.rs](../src/convert.rs).
- `ValidationReport` inspects cached metadata for potential issues. See [src/validation.rs](../src/validation.rs).

### Feature-gated modules
- `async-tokio`: `FrameStream` (background decode thread → mpsc channel → `tokio_stream::Stream`) and `AudioFuture` for non-blocking extraction. See [src/stream.rs](../src/stream.rs).
- `parallel`: `frames_parallel()` distributes frame decoding across rayon threads, each with its own demuxer. See [src/parallel.rs](../src/parallel.rs).
- `hw-accel`: `HwAccelMode`, `HwDeviceType`, and helpers for FFmpeg hardware-accelerated decoding via `ffmpeg_sys_next`. See [src/hwaccel.rs](../src/hwaccel.rs).
- `scene-detection`: `SceneChange` and `SceneDetectionConfig` using FFmpeg's `scdet` filter. See [src/scene.rs](../src/scene.rs).

## Developer workflows
- Build: `cargo build` (FFmpeg dev libraries must be installed; see README).
- Build with all features: `cargo build --all-features`.
- Tests: generate fixtures first (`tests/fixtures/generate_fixtures.sh` or `.bat`), then run `cargo test`.
- Examples: `cargo run --example <name> -- path/to/video.mp4`; example entry points live in [examples/](../examples/).
- Benchmarks: `cargo bench` runs Criterion benchmarks in [benches/](../benches/).

## Project-specific conventions and patterns
- Metadata is extracted once at open; avoid recomputing stream properties if `MediaMetadata` already provides them.
- `MediaMetadata` includes `audio_tracks: Option<Vec<AudioMetadata>>` and `subtitle_tracks: Option<Vec<SubtitleMetadata>>` for multi-track access.
- Frame selection logic prefers sequential decoding when possible; `FrameRange::Specific` sorts/dedups inputs to minimize seeks.
- Timestamp validation is done against `MediaMetadata.duration`; follow this pattern in new range-based APIs.
- Frame conversion uses `frame_to_buffer(bytes_per_pixel)` from utilities with row-stride handling; `FrameOutputConfig` controls pixel format (RGB8/RGBA8/GRAY8) and resolution.
- Audio code uses a `PacketWriter` trait to abstract in-memory vs file output; add new output targets by implementing this trait.
- The `for_each_frame` method provides streaming frame processing without collecting into a `Vec`; prefer it when frames can be processed independently.
- `FrameIterator` provides lazy iteration via `Iterator` trait; it owns a decoder and reads packets one at a time using `Packet::read(&mut Input)`.
- Methods returning `_with_config` variants accept `ExtractionConfig` for progress/cancellation; the original methods delegate to these with default config.
- Async methods (`frames_stream`, `extract_async`) open a fresh demuxer on a blocking thread and release the unbundler borrow immediately.
- Parallel extraction (`frames_parallel`) splits frame numbers into contiguous runs and processes each on a separate rayon thread with its own demuxer.

## Coding conventions
- Public APIs return `Result<T, UnbundleError>` and convert upstream FFmpeg/image errors into `UnbundleError` variants (see [src/error.rs](../src/error.rs)).
- Prefer `MediaUnbundler::metadata()` for stream properties instead of re-reading codec parameters; only decode when needed (see [src/unbundler.rs](../src/unbundler.rs)).
- Use the utilities helpers for timestamp and PTS math rather than inline conversions (see [src/utilities.rs](../src/utilities.rs)).
- Video extraction should build a fresh decoder per call, seek using stream time base, and convert via `frame_to_buffer` / `convert_frame_to_image` (see [src/video.rs](../src/video.rs)).
- Audio in-memory encoding uses FFmpeg dynamic buffer I/O (`avio_open_dyn_buf`/`avio_close_dyn_buf`) via `ffmpeg_sys_next` (see [src/audio.rs](../src/audio.rs)).
- Subtitle decoding uses `avcodec_decode_subtitle2` via `decoder.decode(&packet, &mut subtitle)` — NOT `send_packet`/`receive_frame` (see [src/subtitle.rs](../src/subtitle.rs)).
- Feature-gated code uses `#[cfg(feature = "feature-name")]` on both module declarations in `lib.rs` and on public methods/types.

## Integrations and dependencies
- FFmpeg is required at build/runtime and accessed through `ffmpeg-next` and `ffmpeg-sys-next`; use those crates for all media I/O and encoding.
- `image` is used for `DynamicImage` outputs; avoid introducing alternative image types unless required.
- Errors should be mapped into `UnbundleError` variants instead of bubbling raw FFmpeg errors.
- Optional dependencies: `tokio`/`tokio-stream`/`futures-core` (async), `rayon`/`crossbeam-channel` (parallel).

---

## LLM Coding Guidelines Prompt

The following is a detailed prompt for any LLM (language model) working on the `unbundle` crate. These rules MUST be followed when generating, reviewing, or modifying code.

### 1. Architecture Rules

**1.1 Entry Point Pattern**
- `MediaUnbundler` is the ONLY entry point for opening media files. Never create alternative constructors or bypass this struct.
- When opening a file, metadata is extracted and cached immediately. Do NOT re-extract metadata; always use `unbundler.metadata()`.

**1.2 Extractor Borrowing**
- `VideoExtractor`, `AudioExtractor`, and `SubtitleExtractor` are short-lived, mutable borrows of `MediaUnbundler`.
- You CANNOT hold both extractors simultaneously — this is enforced by Rust's borrow checker.
- Pattern: `unbundler.video().frame(0)` or `unbundler.audio().extract(...)` — call, use, drop.

**1.3 Decoder Lifecycle**
- Video decoders are created fresh for EACH extraction call. Do not cache or reuse decoders across calls.
- Seeking always targets a keyframe first, then decodes forward to the requested frame.

**1.4 Memory vs File Output**
- Audio extraction supports both file and in-memory output.
- In-memory output MUST use FFmpeg's dynamic buffer I/O (`avio_open_dyn_buf` / `avio_close_dyn_buf`) via `ffmpeg_sys_next`.
- Never write to a temp file and read it back for in-memory output.
- The `PacketWriter` trait abstracts packet writing for both output targets; `MemoryPacketWriter` (unsafe, raw `AVFormatContext`) and `FilePacketWriter` (safe, `Output`) implement it.
- When adding new audio output targets, implement the `PacketWriter` trait in `src/audio.rs`.

**1.5 Config Threading**
- `ExtractionConfig` carries progress callbacks, cancellation tokens, pixel format, resolution, and HW acceleration mode through extraction methods.
- Methods named `*_with_config` accept `ExtractionConfig`; convenience methods without `_with_config` delegate with default config.
- `FrameOutputConfig` controls pixel format (`OutputPixelFormat::Rgb8`/`Rgba8`/`Gray8`) and optional resolution settings.

**1.6 Subtitle Decoding**
- Subtitle decoding uses `decoder.decode(&packet, &mut subtitle)` — NOT `send_packet`/`receive_frame`.
- Handles `Rect::Text` and `Rect::Ass` subtitle formats; `Rect::Bitmap` subtitles are skipped.
- ASS tags are stripped via `strip_ass_tags()` to produce clean text output.

**1.7 Format Conversion (Remuxing)**
- `Remuxer` performs lossless container format conversion (no re-encoding).
- Uses `encoder::find(Id::None)` for stream copy mode and resets `codec_tag` for muxer compatibility.
- Builder pattern: `exclude_video()`, `exclude_audio()`, `exclude_subtitles()` to selectively omit streams.

### 2. Error Handling Rules

**2.1 Result Types**
- ALL public APIs MUST return `Result<T, UnbundleError>`.
- Never use `unwrap()` or `expect()` in library code (examples/tests are acceptable).
- Never return raw FFmpeg errors (`ffmpeg::Error`) — always wrap them in `UnbundleError` variants.

**2.2 Error Context**
- `UnbundleError` variants MUST carry context: file paths, frame numbers, timestamps, codec names, etc.
- When creating new error variants, include enough information for the caller to understand what failed and why.

**2.3 Error Conversion**
- Use `.map_err(|e| UnbundleError::VariantName { ... })` to convert upstream errors.
- Implement `From<T>` traits for common error types when appropriate.

### 3. Timestamp and Math Rules

**3.1 Use Utility Functions**
- ALL timestamp conversions MUST go through helpers in `src/utilities.rs`.
- Do NOT perform inline PTS/time-base math like `pts * time_base.num / time_base.den` directly.
- Use `crate::utilities::*` functions for duration-to-PTS, PTS-to-duration, frame-to-timestamp, etc.

**3.2 Time Base Awareness**
- FFmpeg streams have different time bases. Always use the stream's time base for seeking and PTS comparisons.
- When converting between `std::time::Duration` and PTS values, use the utility functions.

**3.3 Frame Indexing**
- Frame numbers are 0-indexed.
- Validate frame numbers against `metadata.video.frame_count` before attempting extraction.

### 4. Frame Extraction Rules

**4.1 FrameRange API**
- Frame selection is centralized in the `FrameRange` enum. Extend this enum for new selection patterns.
- Supported variants: `Range`, `Interval`, `TimeRange`, `TimeInterval`, `Specific`.
- `FrameRange::Specific` sorts and deduplicates frame numbers to minimize seeks.

**4.2 Sequential Decoding Preference**
- When extracting multiple frames, prefer sequential decoding over repeated seeks.
- Seeking is expensive; if frames are close together, decode through rather than seeking to each.

**4.3 Pixel Format Conversion**
- Output pixel format is configurable via `FrameOutputConfig` and `OutputPixelFormat` (defaults to `Rgb8`).
- Supported formats: `Rgb8`, `Rgba8`, `Gray8` — each produces the corresponding `DynamicImage` variant.
- Use `frame_to_buffer(bytes_per_pixel)` from utilities for raw buffer extraction — handles row stride correctly.
- The legacy `frame_to_rgb_buffer` is a thin wrapper around `frame_to_buffer(3)`.
- Never copy planes directly without accounting for stride/padding.

**4.4 Validation**
- Validate timestamps against `metadata.duration` before extraction.
- Return `UnbundleError::FrameOutOfRange` or `UnbundleError::InvalidTimestamp` for invalid inputs.
- Return `UnbundleError::InvalidRange` when range start exceeds end.
- Return `UnbundleError::InvalidInterval` when interval/step is zero.

**4.5 Streaming vs Collecting**
- `frames()` collects all decoded frames into a `Vec<DynamicImage>`.
- `for_each_frame()` processes frames one at a time via a callback without collecting.
- `frame_iter()` returns a `FrameIterator` for lazy, pull-based iteration via Rust's `Iterator` trait.
- Both `frames()` and `for_each_frame()` share the same internal decode logic via `process_frame_range` and `process_specific_frames`.
- `FrameIterator` uses `Packet::read(&mut Input)` for packet-level control, avoiding the borrow conflict with `packets()` iterator.
- Prefer `for_each_frame` when frames can be processed independently (e.g. saving to disk).
- Prefer `frame_iter` when the caller needs control over iteration pace or wants to short-circuit.

**4.6 Async and Parallel Extraction**
- `frames_stream()` (feature `async-tokio`) returns a `FrameStream` implementing `tokio_stream::Stream`.
- Async methods open a fresh demuxer on a `spawn_blocking` thread, releasing the unbundler borrow immediately.
- `frames_parallel()` (feature `parallel`) distributes frame decoding across rayon threads, each with its own demuxer.
- Parallel extraction splits frame numbers into contiguous runs (gap threshold = 30) for efficient sequential decoding per chunk.

### 5. Audio Extraction Rules

**5.1 Format Support**
- Supported formats: `AudioFormat::Wav`, `AudioFormat::Mp3`, `AudioFormat::Flac`, `AudioFormat::Aac`.
- When adding new formats, update the `AudioFormat` enum and encoder selection logic.

**5.2 Range Extraction**
- Audio ranges use `Duration` types for start/end times.
- Validate that `start < end` and both are within `metadata.duration`.

**5.3 Encoder Configuration**
- Use appropriate encoder settings for each format (sample rate, channels, bitrate).
- Preserve original sample rate and channel count when possible.

### 6. Metadata Rules

**6.1 Single Extraction**
- Metadata is extracted ONCE when `MediaUnbundler::open()` is called.
- Never re-read codec parameters or stream info if `MediaMetadata` provides it.

**6.2 Optional Streams**
- `metadata.video`, `metadata.audio`, and `metadata.subtitle` are `Option<T>` — files may lack any stream type.
- `metadata.audio_tracks` and `metadata.subtitle_tracks` are `Option<Vec<T>>` for multi-track access.
- Always check for `None` before accessing stream-specific properties.
- Return `UnbundleError::NoVideoStream`, `UnbundleError::NoAudioStream`, or `UnbundleError::NoSubtitleStream` when the required stream is missing.
- Use `unbundler.audio_track(index)` and `unbundler.subtitle_track(index)` for multi-track extraction.

### 7. Dependency Rules

**7.1 FFmpeg Access**
- Use `ffmpeg-next` for safe Rust bindings.
- Use `ffmpeg_sys_next` ONLY when safe bindings are insufficient (e.g., dynamic buffer I/O).
- Never add alternative media processing libraries.

**7.2 Image Output**
- Use `image::DynamicImage` for frame output.
- Do not introduce alternative image types (e.g., raw buffers, other image crates) unless absolutely necessary.

**7.3 Error Wrapping**
- Always wrap external crate errors into `UnbundleError` variants — never expose raw errors to callers.

### 8. Code Style Rules

**8.1 Imports — CRITICAL**

This crate uses a strict import style. Follow these rules exactly:

**Merge imports from the same parent module using braces:**
```rust
// ✅ CORRECT — merge siblings under the same parent
use std::path::{Path, PathBuf};
use std::time::Duration;

// ❌ WRONG — separate lines for items from the same parent
use std::path::Path;
use std::path::PathBuf;
```

**Nesting inside braces is allowed when items share a parent:**
```rust
// ✅ CORRECT — nesting different depth levels
use std::{io, fs, path::Path};
```

**Three groups, separated by blank lines:**
1. `std` imports (standard library)
2. External crate imports (third-party)
3. `crate::` imports (this crate's modules)

```rust
// ✅ CORRECT — three groups with blank lines, siblings merged
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::time::Duration;

use ffmpeg_next::{ChannelLayout, Packet, Rational};
use ffmpeg_next::codec::Id;
use ffmpeg_next::codec::context::Context as CodecContext;
use image::{DynamicImage, RgbImage};

use crate::error::UnbundleError;
use crate::metadata::{MediaMetadata, VideoMetadata};
use crate::unbundler::MediaUnbundler;
```

**Alphabetical ordering within each group:**
- Sort by full path, not just the final item name
- `std::io` comes before `std::path`
- `ffmpeg_next::codec` comes before `ffmpeg_next::format`

**Use `as` for type aliasing when names collide or are generic:**
```rust
use ffmpeg_next::codec::context::Context as CodecContext;
use ffmpeg_next::decoder::Audio as AudioDecoder;
use ffmpeg_next::frame::Video as VideoFrame;
use ffmpeg_next::software::scaling::{Context as ScalingContext, Flags as ScalingFlags};
```

**Always use `crate::` for internal imports, never `super::`:**
```rust
// ✅ CORRECT
use crate::error::UnbundleError;
use crate::metadata::VideoMetadata;

// ❌ WRONG
use super::error::UnbundleError;
use super::*;
```

**Never use glob imports (`*`):**
```rust
// ❌ WRONG — never use wildcards
use std::io::*;
use crate::*;
```

**8.2 What to Import vs. What to Fully Qualify — CRITICAL**

This crate has strict rules about WHAT gets imported and WHAT gets called with full paths.

**IMPORT: Types (structs) — always import the type itself:**
```rust
// ✅ CORRECT — import the type, use its methods directly
use std::time::Duration;
use std::path::PathBuf;
use image::DynamicImage;

let d = Duration::from_secs(5);        // Method on type
let p = PathBuf::from("/tmp");         // Method on type
```

**IMPORT: Enums — import the enum, NOT individual variants:**
```rust
// ✅ CORRECT — import enum, qualify variants
use crate::error::UnbundleError;
use crate::video::FrameRange;
use crate::audio::AudioFormat;

return Err(UnbundleError::NoVideoStream);        // Qualified variant
let range = FrameRange::Interval(30);            // Qualified variant
let fmt = AudioFormat::Wav;                      // Qualified variant

// ❌ WRONG — never import enum variants directly
use crate::error::UnbundleError::NoVideoStream;  // NO!
use crate::audio::AudioFormat::*;                // NO!
```

**DO NOT IMPORT: Freestanding functions — call them fully qualified:**
```rust
// ✅ CORRECT — call with full crate path, no import
let buffer = crate::utilities::frame_to_rgb_buffer(frame, width, height);
let ts = crate::utilities::duration_to_stream_timestamp(duration, time_base);
let frame_num = crate::utilities::timestamp_to_frame_number(timestamp, fps);

// ❌ WRONG — never import freestanding functions
use crate::utilities::frame_to_rgb_buffer;       // NO!
use crate::utilities::*;                         // NO!
frame_to_rgb_buffer(frame, width, height);       // NO! (unqualified call)
```

**DO NOT IMPORT: Macros — call them fully qualified:**
```rust
// ✅ CORRECT — macros are called with their full crate path
criterion::criterion_group!(benches, bench_fn);
criterion::criterion_main!(benches);

// ❌ WRONG — never import macros
use criterion::criterion_group;
use criterion::criterion_main;
criterion_group!(benches, bench_fn);  // NO! (unqualified call)
```

**Summary table:**

| Item Type              | Import?      | Usage Pattern                                    |
|------------------------|--------------|--------------------------------------------------|
| Struct/Type            | ✅ Yes       | `Duration::from_secs(5)`                         |
| Enum                   | ✅ Yes       | `UnbundleError::NoVideoStream`                   |
| Enum Variant           | ❌ No        | Always qualify: `Enum::Variant`                  |
| Freestanding Function  | ❌ No        | Always qualify: `crate::module::function()`      |
| Macro                  | ❌ No        | Always qualify: `crate_name::macro!()`           |
| Trait                  | ✅ Yes       | Import to bring methods into scope               |
| Associated Function    | N/A          | Call via type: `Type::function()`                |

**8.3 Documentation**
- All public items MUST have doc comments (`///`).
- Include `# Example` sections with `no_run` code blocks for complex APIs.
- Include `# Errors` sections listing possible error variants.

**8.4 Testing**
- Integration tests live in `tests/` and require fixture files.
- Tests should skip gracefully if fixtures are missing (check with `Path::new(...).exists()`).
- Benchmarks use Criterion and live in `benches/`.

### 9. Feature-Gated Code Rules

**9.1 Feature Flags**
- Feature-gated code uses `#[cfg(feature = "feature-name")]` on both module declarations in `lib.rs` and on public methods/types.
- Available features: `async-tokio`, `parallel`, `hw-accel`, `scene-detection`, `full` (enables all).
- Default features are empty — the crate compiles with no optional dependencies by default.

**9.2 Async (`async-tokio`)**
- `FrameStream` wraps `mpsc::Receiver` + `JoinHandle`, implements `tokio_stream::Stream`.
- `AudioFuture` wraps `JoinHandle`, implements `std::future::Future`.
- Async methods open a fresh demuxer on a blocking thread; the unbundler borrow is released immediately.

**9.3 Parallel (`parallel`)**
- `frames_parallel()` splits frame numbers into contiguous runs and processes each on a rayon thread.
- Each thread opens its own `MediaUnbundler` instance to avoid `Send`/`Sync` issues with `Input`.

**9.4 HW Acceleration (`hw-accel`)**
- `HwAccelMode` and `HwDeviceType` control hardware-accelerated decoding.
- Uses unsafe `ffmpeg_sys_next` for `av_hwdevice_ctx_create`, `av_hwframe_transfer_data`, etc.
- `ExtractionConfig::with_hw_accel()` threads HW mode through extraction methods.

**9.5 Scene Detection (`scene-detection`)**
- Uses FFmpeg's `scdet` filter graph for scene change detection.
- Reads `lavfi.scd.score` from frame side data via unsafe `av_dict_get`.

### 10. Validation and Conversion Rules

**10.1 Validation**
- `ValidationReport` inspects cached metadata for potential issues (no additional I/O).
- Reports are categorized into info, warnings, and errors.
- `is_valid()` returns true only when the errors list is empty.

**10.2 Remuxing**
- `Remuxer` copies packets without re-encoding — timestamps are rescaled between stream time bases.
- Always reset `codec_tag` to 0 to let the output muxer choose the correct tag.
- Use builder methods to selectively exclude video, audio, or subtitle streams.

### 11. Summary Checklist

When writing or reviewing code for `unbundle`, verify:

- [ ] All public functions return `Result<T, UnbundleError>`
- [ ] Errors include context (paths, frame numbers, timestamps)
- [ ] Timestamp math uses `utilities.rs` helpers, not inline calculations
- [ ] Frame extraction creates a fresh decoder per call
- [ ] Metadata is accessed via `unbundler.metadata()`, not re-extracted
- [ ] Frame output respects `FrameOutputConfig` (pixel format, resolution)
- [ ] Optional streams (`video`/`audio`/`subtitle`) are checked before use
- [ ] No raw FFmpeg errors are returned to callers
- [ ] Doc comments exist for all public items
- [ ] Feature-gated code has `#[cfg(feature = "...")]` on both modules and public items
- [ ] `_with_config` variants accept `ExtractionConfig`; convenience methods delegate with defaults
- [ ] Async/parallel operations open fresh demuxers, not shared contexts
- [ ] Cancellation checks appear in all decode loops
