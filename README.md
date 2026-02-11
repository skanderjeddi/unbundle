# unbundle

[![Crates.io](https://img.shields.io/crates/v/unbundle)](https://crates.io/crates/unbundle)
[![docs.rs](https://img.shields.io/docsrs/unbundle)](https://docs.rs/unbundle)
[![License: MIT](https://img.shields.io/crates/l/unbundle)](LICENSE)

Unbundle media files — extract still frames, audio tracks, and subtitles from
video files.

`unbundle` provides a clean, ergonomic Rust API for extracting video frames as
[`image::DynamicImage`](https://docs.rs/image/latest/image/enum.DynamicImage.html)
values, audio tracks as encoded byte vectors, and subtitle tracks as structured
text, powered by FFmpeg via
[`ffmpeg-next`](https://crates.io/crates/ffmpeg-next).

## Features

- **Frame extraction** — by frame number, timestamp, range, interval, or
  specific frame list
- **Audio extraction** — to WAV, MP3, FLAC, or AAC (file or in-memory)
- **Subtitle extraction** — decode text-based subtitles to SRT, WebVTT, or raw
  text
- **Container remuxing** — lossless format conversion (e.g. MKV → MP4) without
  re-encoding
- **Rich metadata** — video dimensions, frame rate, frame count, audio sample
  rate, channels, codec info, multi-track audio/subtitle metadata
- **Configurable output** — pixel format (RGB8, RGBA8, GRAY8), target
  resolution with aspect ratio preservation
- **Progress & cancellation** — cooperative progress callbacks and
  `CancellationToken` for long-running operations
- **Streaming iteration** — lazy `FrameIterator` (pull-based) and
  `for_each_frame` (push-based) without buffering entire frame sets
- **Validation** — inspect media files for structural issues before extraction
- **Chapter support** — extract chapter metadata (titles, timestamps) from
  containers
- **Frame metadata** — per-frame decode info (PTS, keyframe flag, picture type)
  via `frame_with_info` / `frames_with_info`
- **Segmented extraction** — extract frames from multiple disjoint time ranges
  in a single call with `FrameRange::Segments`
- **Stream probing** — lightweight `MediaProbe` for quick metadata inspection
  without keeping the demuxer open
- **Thumbnail helpers** — single-frame thumbnails, contact-sheet grids, and
  variance-based "smart" thumbnail selection
- **Efficient seeking** — seeks to the nearest keyframe, then decodes forward
- **Zero-copy in-memory audio** — uses FFmpeg's dynamic buffer I/O

### Optional Features (feature flags)

| Feature | Description |
|---------|-------------|
| `async-tokio` | `FrameStream` (async frame iteration) and `AudioFuture` via Tokio |
| `parallel` | `frames_parallel()` distributes decoding across rayon threads |
| `hw-accel` | Hardware-accelerated decoding (CUDA, VAAPI, DXVA2, D3D11VA, VideoToolbox, QSV) |
| `scene-detection` | Scene change detection via FFmpeg's `scdet` filter |
| `full` | Enables all of the above |

```toml
[dependencies]
unbundle = { version = "1.1", features = ["full"] }
```

## Installation

Add `unbundle` to your `Cargo.toml`:

```toml
[dependencies]
unbundle = "1.1"
```

### System Requirements

`unbundle` links against FFmpeg's native libraries via `ffmpeg-next`. You must
have the FFmpeg development headers and libraries installed.

**Linux (Debian/Ubuntu):**

```bash
sudo apt-get install libavcodec-dev libavformat-dev libavutil-dev \
    libswscale-dev libswresample-dev libavdevice-dev pkg-config
```

**macOS:**

```bash
brew install ffmpeg pkg-config
```

**Windows:**

Download FFmpeg development builds from <https://ffmpeg.org/download.html> or
use vcpkg:

```powershell
vcpkg install ffmpeg:x64-windows
```

Set the `FFMPEG_DIR` environment variable to point to your FFmpeg installation.

## Quick Start

### Extract Video Frames

```rust
use std::time::Duration;

use unbundle::MediaUnbundler;

let mut unbundler = MediaUnbundler::open("input.mp4")?;

// Extract the first frame
let frame = unbundler.video().frame(0)?;
frame.save("first_frame.png")?;

// Extract a frame at 30 seconds
let frame = unbundler.video().frame_at(Duration::from_secs(30))?;
frame.save("frame_30s.png")?;
```

### Extract Multiple Frames

```rust
use std::time::Duration;

use unbundle::{FrameRange, MediaUnbundler};

let mut unbundler = MediaUnbundler::open("input.mp4")?;

// Every 30th frame
let frames = unbundler.video().frames(FrameRange::Interval(30))?;

// Frames between two timestamps
let frames = unbundler.video().frames(
    FrameRange::TimeRange(Duration::from_secs(10), Duration::from_secs(20))
)?;

// Specific frame numbers
let frames = unbundler.video().frames(
    FrameRange::Specific(vec![0, 50, 100, 150])
)?;
```

### Streaming Frame Iteration

```rust
use unbundle::{FrameRange, MediaUnbundler};

let mut unbundler = MediaUnbundler::open("input.mp4")?;

// Push-based: process each frame without buffering
unbundler.video().for_each_frame(
    FrameRange::Range(0, 99),
    |frame_number, image| {
        image.save(format!("frame_{frame_number}.png"))?;
        Ok(())
    },
)?;

// Pull-based: lazy iterator with early exit
let iter = unbundler.video().frame_iter(FrameRange::Range(0, 99))?;
for result in iter {
    let (frame_number, image) = result?;
    image.save(format!("frame_{frame_number}.png"))?;
}
```

### Extract Audio

```rust
use std::time::Duration;

use unbundle::{AudioFormat, MediaUnbundler};

let mut unbundler = MediaUnbundler::open("input.mp4")?;

// Save complete audio track to WAV
unbundler.audio().save("output.wav", AudioFormat::Wav)?;

// Extract a 30-second segment as MP3
unbundler.audio().save_range(
    "segment.mp3",
    Duration::from_secs(30),
    Duration::from_secs(60),
    AudioFormat::Mp3,
)?;

// Extract audio to memory
let audio_bytes = unbundler.audio().extract(AudioFormat::Wav)?;

// Multi-track: extract the second audio track
let audio_bytes = unbundler.audio_track(1)?.extract(AudioFormat::Wav)?;
```

### Extract Subtitles

```rust
use unbundle::{MediaUnbundler, SubtitleFormat};

let mut unbundler = MediaUnbundler::open("input.mkv")?;

// Extract subtitle entries with timing
let entries = unbundler.subtitle().extract()?;
for entry in &entries {
    println!("[{:?} → {:?}] {}", entry.start_time, entry.end_time, entry.text);
}

// Save as SRT file
unbundler.subtitle().save("output.srt", SubtitleFormat::Srt)?;

// Multi-track: extract the second subtitle track
unbundler.subtitle_track(1)?.save("track2.vtt", SubtitleFormat::WebVtt)?;
```

### Container Remuxing

```rust
use unbundle::Remuxer;

// Convert MKV to MP4 without re-encoding
Remuxer::new("input.mkv", "output.mp4")?.run()?;

// Exclude subtitles during remux
Remuxer::new("input.mkv", "output.mp4")?
    .exclude_subtitles()
    .run()?;
```

### Progress & Cancellation

```rust
use std::sync::Arc;

use unbundle::{
    CancellationToken, ExtractionConfig, FrameRange,
    MediaUnbundler, ProgressCallback, ProgressInfo,
};

struct PrintProgress;
impl ProgressCallback for PrintProgress {
    fn on_progress(&self, info: &ProgressInfo) {
        println!("Frame {}/{}", info.current, info.total.unwrap_or(0));
    }
}

let token = CancellationToken::new();
let config = ExtractionConfig::new()
    .with_progress(Arc::new(PrintProgress))
    .with_cancellation(token.clone());

let mut unbundler = MediaUnbundler::open("input.mp4")?;
let frames = unbundler.video().frames_with_config(
    FrameRange::Range(0, 99),
    &config,
)?;
```

### Custom Output Format

```rust
use unbundle::{ExtractionConfig, FrameRange, MediaUnbundler, OutputPixelFormat};

let config = ExtractionConfig::new()
    .with_pixel_format(OutputPixelFormat::Rgba8)
    .with_resolution(1280, 720);

let mut unbundler = MediaUnbundler::open("input.mp4")?;
let frames = unbundler.video().frames_with_config(
    FrameRange::Interval(30),
    &config,
)?;
```

### Read Metadata

```rust
use unbundle::MediaUnbundler;

let unbundler = MediaUnbundler::open("input.mp4")?;
let metadata = unbundler.metadata();

println!("Duration: {:?}", metadata.duration);
println!("Format: {}", metadata.format);

if let Some(video) = &metadata.video {
    println!("Video: {}x{}, {:.2} fps, {} frames",
        video.width, video.height,
        video.frames_per_second, video.frame_count);
    println!("Codec: {}", video.codec);
}

if let Some(audio) = &metadata.audio {
    println!("Audio: {} Hz, {} channels, codec: {}",
        audio.sample_rate, audio.channels, audio.codec);
}

// List all audio and subtitle tracks
if let Some(tracks) = &metadata.audio_tracks {
    println!("{} audio track(s)", tracks.len());
}
if let Some(tracks) = &metadata.subtitle_tracks {
    println!("{} subtitle track(s)", tracks.len());
}
```

### Validate Media Files

```rust
use unbundle::MediaUnbundler;

let unbundler = MediaUnbundler::open("input.mp4")?;
let report = unbundler.validate();

if report.is_valid() {
    println!("File is valid");
} else {
    println!("{report}");
}
```

### Probe Media Files

```rust
use unbundle::MediaProbe;

// Quick metadata inspection without keeping the file open
let metadata = MediaProbe::probe("input.mp4")?;
println!("Duration: {:?}", metadata.duration);

// Probe multiple files at once
let results = MediaProbe::probe_many(&["video1.mp4", "video2.mkv"]);
```

### Chapter Metadata

```rust
use unbundle::MediaUnbundler;

let unbundler = MediaUnbundler::open("input.mkv")?;
let metadata = unbundler.metadata();

if let Some(chapters) = &metadata.chapters {
    for chapter in chapters {
        println!("[{:?} → {:?}] {}",
            chapter.start, chapter.end,
            chapter.title.as_deref().unwrap_or("Untitled"));
    }
}
```

### Frame Metadata

```rust
use unbundle::MediaUnbundler;

let mut unbundler = MediaUnbundler::open("input.mp4")?;

// Get a frame with its decode metadata
let (image, info) = unbundler.video().frame_with_info(0)?;
println!("Frame {}: keyframe={}, type={:?}, pts={:?}",
    info.frame_number, info.is_keyframe, info.frame_type, info.pts);
```

### Thumbnail Generation

```rust
use std::time::Duration;

use unbundle::{MediaUnbundler, ThumbnailConfig, ThumbnailGenerator};

let mut unbundler = MediaUnbundler::open("input.mp4")?;

// Single thumbnail at a timestamp
let thumb = ThumbnailGenerator::at_timestamp(&mut unbundler, Duration::from_secs(5), 320)?;

// Contact-sheet grid
let config = ThumbnailConfig::new(4, 3); // 4 columns × 3 rows
let grid = ThumbnailGenerator::grid(&mut unbundler, &config)?;
grid.save("contact_sheet.png")?;

// Smart thumbnail (picks frame with highest visual variance)
let smart = ThumbnailGenerator::smart(&mut unbundler, 10, 320)?;
```

## API Documentation

See the [API docs](https://docs.rs/unbundle) for complete documentation.

### Core Types

| Type | Description |
|------|-------------|
| `MediaUnbundler` | Main entry point — opens a media file and provides access to extractors |
| `VideoExtractor` | Extracts video frames as `DynamicImage` |
| `AudioExtractor` | Extracts audio tracks as bytes or files |
| `SubtitleExtractor` | Extracts text-based subtitle tracks |
| `Remuxer` | Lossless container format conversion |
| `FrameRange` | Specifies which frames to extract (range, interval, timestamps, etc.) |
| `FrameIterator` | Lazy, pull-based frame iterator |
| `AudioFormat` | Output audio format (WAV, MP3, FLAC, AAC) |
| `SubtitleFormat` | Output subtitle format (SRT, WebVTT, Raw) |
| `ExtractionConfig` | Threading progress callbacks, cancellation, pixel format, resolution, HW accel |
| `ValidationReport` | Result of media file validation |
| `MediaMetadata` | Container-level metadata (duration, format) |
| `VideoMetadata` | Video stream metadata (dimensions, frame rate, codec) |
| `AudioMetadata` | Audio stream metadata (sample rate, channels, codec) |
| `SubtitleMetadata` | Subtitle stream metadata (codec, language) |
| `ProgressCallback` | Trait for receiving progress updates |
| `ProgressInfo` | Progress event data (current, total, percentage, ETA) |
| `CancellationToken` | Cooperative cancellation via `Arc<AtomicBool>` |
| `OperationType` | Identifies the operation being tracked |
| `UnbundleError` | Error type with rich context |
| `FrameInfo` | Per-frame decode metadata (PTS, keyframe flag, picture type) |
| `FrameType` | Picture type enum (I, P, B, etc.) |
| `ChapterMetadata` | Chapter information (title, start/end times) |
| `MediaProbe` | Lightweight stateless media file probing |
| `ThumbnailGenerator` | Thumbnail generation helpers (single, grid, smart) |
| `ThumbnailConfig` | Grid thumbnail configuration (columns, rows, width) |

### Feature-Gated Types

| Type | Feature | Description |
|------|---------|-------------|
| `FrameStream` | `async-tokio` | Async stream of decoded frames via Tokio |
| `AudioFuture` | `async-tokio` | Async audio extraction future |
| `HwAccelMode` | `hw-accel` | Hardware acceleration mode selection |
| `HwDeviceType` | `hw-accel` | Supported HW device types (CUDA, VAAPI, etc.) |
| `SceneChange` | `scene-detection` | Detected scene change with timestamp and score |
| `SceneDetectionConfig` | `scene-detection` | Scene detection threshold configuration |

## Examples

See the [`examples/`](https://github.com/skanderjeddi/unbundle/tree/main/examples) directory:

| Example | Description |
|---------|-------------|
| `extract_frames` | Extract frames by number, timestamp, range, interval |
| `extract_audio` | Extract the complete audio track |
| `extract_audio_segment` | Extract a specific time range as MP3 |
| `thumbnail_grid` | Create a thumbnail grid from evenly-spaced frames |
| `metadata` | Display all media metadata |
| `frame_iterator` | Lazy frame iteration with early exit |
| `pixel_formats` | Demonstrate RGB8/RGBA8/GRAY8 output |
| `progress` | Progress callbacks and cancellation |
| `subtitles` | Extract subtitles as SRT/WebVTT/raw text |
| `remux` | Lossless container format conversion |
| `validate` | Media file validation report |

Run an example:

```bash
cargo run --example metadata -- path/to/video.mp4
```

## Performance

- **Seeking:** Uses FFmpeg's keyframe-based seeking. For sequential access
  (ranges, intervals), frames are decoded without redundant seeks.
- **Decoder lifecycle:** Each extraction call creates a fresh, lightweight
  decoder. FFmpeg decoder creation is fast relative to actual decoding.
- **Batch optimisation:** `FrameRange::Specific` sorts requested frame numbers
  and processes them in order to minimise seeks.
- **Streaming:** `for_each_frame` and `FrameIterator` process frames one at a
  time without buffering the entire frame set.
- **Parallel extraction:** `frames_parallel()` (feature `parallel`) splits
  frames across rayon threads, each with its own demuxer.
- **Hardware acceleration:** When enabled (feature `hw-accel`), the decoder
  attempts GPU-accelerated decoding with automatic fallback to software.
- **Stride handling:** Correctly handles FFmpeg's row padding when converting
  frames to `image` buffers.
- **In-memory audio:** Uses `avio_open_dyn_buf` for zero-copy in-memory audio
  encoding without temporary files.

## Testing

Generate test fixtures first:

```bash
# Linux / macOS
bash tests/fixtures/generate_fixtures.sh

# Windows
tests\fixtures\generate_fixtures.bat
```

Then run tests:

```bash
cargo test --all-features
```

### Test Suites

| Test file | Coverage |
|-----------|----------|
| `video_extraction` | Single frames, ranges, intervals, timestamps, specific lists, pixel formats, resolution scaling |
| `audio_extraction` | WAV/MP3/FLAC/AAC extraction, ranges, file output, multi-track |
| `subtitle_extraction` | Subtitle decoding, SRT/WebVTT export, multi-track |
| `metadata` | Container metadata, video/audio/subtitle stream properties |
| `config` | ExtractionConfig builder, pixel formats, resolution, cancellation |
| `progress` | ProgressCallback, ProgressInfo fields, CancellationToken |
| `error_handling` | Error variants, context, invalid inputs, missing streams |
| `frame_iterator` | FrameIterator, lazy iteration, early exit |
| `conversion` | Remuxer, stream exclusion, lossless format conversion |
| `validation` | ValidationReport, warnings, errors, valid files |
| `scene_detection` | Scene change detection, threshold configuration |
| `chapters` | Chapter metadata extraction, titles, timestamps, ordering |
| `frame_metadata` | FrameInfo, FrameType, keyframe detection, PTS values |
| `segmented_extraction` | FrameRange::Segments, multiple disjoint time ranges |
| `probing` | MediaProbe, probe/probe_many, error handling |
| `thumbnail` | ThumbnailGenerator, grid, smart selection, aspect ratio |

## Benchmarks

Criterion benchmarks live in `benches/`:

```bash
cargo bench --all-features
```

## License

MIT
