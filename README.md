# unbundle

[![Crates.io](https://img.shields.io/crates/v/unbundle)](https://crates.io/crates/unbundle)
[![docs.rs](https://img.shields.io/docsrs/unbundle)](https://docs.rs/unbundle)
[![CI](https://github.com/skanderjeddi/unbundle/actions/workflows/ci.yml/badge.svg)](https://github.com/skanderjeddi/unbundle/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/crates/l/unbundle)](LICENSE)

A clean, ergonomic Rust library for extracting video frames, audio tracks, and subtitles from media files using FFmpeg.

```rust
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("video.mp4")?;

// Extract a frame at 30 seconds
let frame = unbundler.video().frame_at(Duration::from_secs(30))?;
frame.save("frame_30s.png")?;

// Extract complete audio track
unbundler.audio().save("audio.wav", AudioFormat::Wav)?;
```

## Why unbundle?

- **Type-safe API** — frames as [`image::DynamicImage`](https://docs.rs/image/latest/image/enum.DynamicImage.html), audio as bytes or files, subtitles as structured events
- **Flexible extraction** — by frame number, timestamp, range, interval, or custom frame lists
- **Streaming support** — lazy iterators and async streams avoid buffering entire frame sets
- **Rich metadata** — dimensions, frame rates, codecs, chapters, per-frame decode info
- **Production-ready** — progress callbacks, cancellation tokens, hardware acceleration, parallel processing

## Use Cases

- **Video thumbnails** — contact sheets, smart frame selection, chapter previews
- **Media processing** — format conversion, audio extraction, subtitle manipulation
- **Analysis tools** — scene detection, keyframe analysis, Group of Pictures structure inspection
- **Content indexing** — frame extraction for search, waveform visualization
- **Transcoding pipelines** — lossless remuxing, audio re-encoding

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
unbundle = "4.3.5"
```

Or with additional features:

```toml
[dependencies]
unbundle = { version = "4.3.5", features = ["async", "rayon", "hardware"] }
```

### System Requirements

`unbundle` requires FFmpeg libraries (4.0+) installed on your system.

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

Use vcpkg (recommended for headers + libs):

```powershell
# Install vcpkg if needed
git clone https://github.com/microsoft/vcpkg.git C:\vcpkg
C:\vcpkg\bootstrap-vcpkg.bat

# Install FFmpeg for MSVC x64
vcpkg install ffmpeg:x64-windows

# Configure environment for build scripts
setx VCPKG_ROOT "C:\vcpkg"
setx VCPKGRS_DYNAMIC "1"
setx FFMPEG_DIR "C:\vcpkg\installed\x64-windows"
```

Then restart your terminal and run `cargo build`.

## Quick Start

### Extract Video Frames

```rust
use std::time::Duration;
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("input.mp4")?;

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
use unbundle::{FrameRange, MediaFile};

let mut unbundler = MediaFile::open("input.mp4")?;

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

### Open from URLs or Streams

```rust
use unbundle::MediaFile;

// Works with paths, file:// URLs, and network URLs supported by your FFmpeg build.
let mut unbundler = MediaFile::open_url("https://example.com/video.mp4")?;
let metadata = unbundler.metadata();
println!("Format: {}", metadata.format);
```

`open_url()` accepts any FFmpeg input string, including `http://`, `https://`, `rtsp://`, and local path-like sources.

### Apply FFmpeg Filters to Frames

```rust
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("input.mp4")?;

// Resize + adjust contrast in one FFmpeg filter graph.
let frame = unbundler
    .video()
    .frame_with_filter(0, "scale=320:240,eq=contrast=1.1")?;

frame.save("filtered.png")?;
```

### Streaming Frame Iteration

```rust
use unbundle::{FrameRange, MediaFile};

let mut unbundler = MediaFile::open("input.mp4")?;

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
use unbundle::{AudioFormat, MediaFile};

let mut unbundler = MediaFile::open("input.mp4")?;

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
use unbundle::{MediaFile, SubtitleFormat};

let mut unbundler = MediaFile::open("input.mkv")?;

// Extract subtitle events with timing
let events = unbundler.subtitle().extract()?;
for event in &events {
    println!("[{:?} → {:?}] {}", event.start_time, event.end_time, event.text);
}

// Save as SRT file
unbundler.subtitle().save("output.srt", SubtitleFormat::Srt)?;

// Multi-track: extract the second subtitle track
unbundler.subtitle_track(1)?.save("track2.vtt", SubtitleFormat::WebVtt)?;
```

### Raw Stream Copy (No Re-encode)

```rust
use std::time::Duration;
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("input.mp4")?;

// Copy video packets exactly as-is (codec preserved)
unbundler.video().stream_copy("video_copy.mp4")?;

// Copy a time segment without decoding/re-encoding
unbundler.video().stream_copy_range(
    "segment.mp4",
    Duration::from_secs(10),
    Duration::from_secs(20),
)?;

// In-memory stream copy
let mkv_bytes = unbundler.video().stream_copy_to_memory("matroska")?;

// Audio stream copy (codec preserved)
unbundler.audio().stream_copy("audio_copy.aac")?;
let adts_bytes = unbundler.audio().stream_copy_to_memory("adts")?;

// Subtitle stream copy (codec preserved)
unbundler.subtitle().stream_copy("subs_copy.mkv")?;
let subtitle_bytes = unbundler.subtitle().stream_copy_to_memory("matroska")?;
```

### Advanced Filter Graph Recipes

```rust
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("input.mp4")?;

// Complex chain: resize -> crop -> mirror -> color adjust
let frame = unbundler.video().frame_with_filter(
    42,
    "scale=640:480,crop=320:240:10:20,hflip,transpose=1",
)?;
frame.save("filtered_advanced.png")?;

// Another chain: downscale + grayscale + mirror
let frame = unbundler
    .video()
    .frame_with_filter(42, "scale=iw/2:ih/2,format=gray,hflip")?;
frame.save("filtered_denoise_gray.png")?;
```

### Progress & Cancellation

```rust
use std::sync::Arc;
use unbundle::{
    CancellationToken, ExtractOptions, FrameRange,
    MediaFile, ProgressCallback, ProgressInfo,
};

struct PrintProgress;
impl ProgressCallback for PrintProgress {
    fn on_progress(&self, info: &ProgressInfo) {
        println!("Frame {}/{}", info.current, info.total.unwrap_or(0));
    }
}

let token = CancellationToken::new();
let config = ExtractOptions::new()
    .with_progress(Arc::new(PrintProgress))
    .with_cancellation(token.clone());

let mut unbundler = MediaFile::open("input.mp4")?;
let frames = unbundler.video().frames_with_options(
    FrameRange::Range(0, 99),
    &config,
)?;
```

## Features

### Core Capabilities

- **Frame extraction** — by frame number, timestamp, range, interval, or specific frame list
- **Audio extraction** — to WAV, MP3, FLAC, or AAC (file or in-memory)
- **Subtitle extraction** — decode text-based subtitles to SRT, WebVTT, or raw text
- **Container remuxing** — lossless format conversion (e.g. MKV → MP4) without re-encoding
- **Rich metadata** — video dimensions, frame rate, frame count, audio sample rate, channels, codec info, multi-track audio/subtitle metadata
- **Configurable output** — pixel format (RGB8, RGBA8, GRAY8), target resolution with aspect ratio preservation
- **Custom FFmpeg filters** — apply filter graphs during frame extraction (e.g. scale, crop, eq, hflip)
- **Progress & cancellation** — cooperative progress callbacks and `CancellationToken` for long-running operations
- **Streaming iteration** — lazy `FrameIterator` (pull-based) and `for_each_frame` (push-based) without buffering entire frame sets
- **Audio sample iteration** — lazy `AudioIterator` yields mono f32 chunks for incremental audio processing
- **Validation** — inspect media files for structural issues before extraction
- **Chapter support** — extract chapter metadata (titles, timestamps) from containers
- **Frame metadata** — per-frame decode info (PTS, keyframe flag, picture type) via `frame_and_metadata` / `frames_and_metadata`
- **Segmented extraction** — extract frames from multiple disjoint time ranges in a single call with `FrameRange::Segments`
- **Stream probing** — lightweight `MediaProbe` for quick metadata inspection without keeping the demuxer open
- **Thumbnail helpers** — single-frame thumbnails, contact-sheet grids, and variance-based "smart" thumbnail selection
- **Keyframe & Group of Pictures analysis** — scan video packets for keyframe positions and Group of Pictures structure without decoding
- **VFR detection** — detect variable frame rate streams and analyze PTS distributions
- **Packet iteration** — raw packet-level demuxer iteration for advanced inspection
- **Raw stream copy** — copy video/audio/subtitle packets directly to file or memory without re-encoding
- **Efficient seeking** — seeks to the nearest keyframe, then decodes forward
- **Zero-copy in-memory audio** — uses FFmpeg's dynamic buffer I/O

### Optional Features

Enable additional functionality through Cargo features:

| Feature | Description |
|---------|-------------|
| `async` | `FrameStream` (async frame iteration) and `AudioFuture` via Tokio |
| `rayon` | `frames_parallel()` distributes decoding across rayon threads |
| `hardware` | Hardware-accelerated decoding (CUDA, VAAPI, DXVA2, D3D11VA, VideoToolbox, QSV) |
| `scene` | Scene change detection via FFmpeg's `scdet` filter |
| `gif` | Animated GIF export from video frames |
| `waveform` | Audio waveform visualization data (min/max/RMS per bin) |
| `loudness` | Peak/RMS loudness analysis with dBFS conversion |
| `transcode` | Audio re-encoding between formats (e.g. AAC → MP3) |
| `encode` | Encode `DynamicImage` sequences into video files (H.264, H.265, MPEG-4) |
| `full` | Enables all of the above |

```toml
[dependencies]
unbundle = { version = "4.3.5", features = ["full"] }
```

#### Feature Usage Guide

- **Use `async`** when integrating with async web servers or when processing multiple videos concurrently
- **Use `rayon`** for CPU-intensive batch frame extraction (e.g., generating thousands of thumbnails)
- **Use `hardware`** when processing high-resolution video (4K+) or when CPU is a bottleneck
- **Use `scene`** for video analysis, automatic chapter detection, or intelligent thumbnail selection
- **Use `gif`** for creating preview animations or social media content
- **Use `waveform` and `loudness`** for audio visualization or normalization workflows
- **Use `transcode`** for audio format conversion in media pipelines
- **Use `encode`** for creating time-lapses, slideshows, or re-encoding frame sequences

## Examples

The [`examples/`](https://github.com/skanderjeddi/unbundle/tree/main/examples) directory contains complete, runnable examples:

| Example | Description |
|---------|-------------|
| `extract_frames` | Extract frames by number, timestamp, range, interval |
| `extract_audio` | Extract the complete audio track |
| `extract_audio_segment` | Extract a specific time range as MP3 |
| `open_url` | Open from URL/path-like source strings |
| `thumbnail` | Create a thumbnail grid from evenly-spaced frames |
| `metadata` | Display all media metadata |
| `video_iterator` | Lazy frame iteration with early exit |
| `pixel_formats` | Demonstrate RGB8/RGBA8/GRAY8 output |
| `progress` | Progress callbacks and cancellation |
| `subtitle` | Extract subtitles as SRT/WebVTT/raw text |
| `remux` | Lossless container format conversion |
| `validate` | Media file validation report |
| `async_extraction` | Async frame streaming and audio extraction (`async`) |
| `rayon` | Parallel frame extraction across threads (`rayon`) |
| `scene` | Scene change detection (`scene`) |
| `hardware_acceleration` | Hardware-accelerated decoding (`hardware`) |
| `gif_export` | Export video frames as animated GIF (`gif`) |
| `waveform` | Generate audio waveform data (`waveform`) |
| `loudness` | Analyze audio loudness levels (`loudness`) |
| `audio_iterator` | Lazy audio sample iteration |
| `video_encoder` | Encode image sequences into video files (`encode`) |
| `transcode` | Re-encode audio between formats (`transcode`) |
| `keyframe` | Group of Pictures/keyframe structure analysis |
| `variable_framerate` | Variable frame rate detection |
| `packet_iterator` | Raw packet-level demuxer inspection |
| `subtitle_search` | Search subtitle text content |

Run an example:

```bash
cargo run --example metadata -- path/to/video.mp4
```

## Advanced Usage

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

### Custom Output Format

```rust
use unbundle::{ExtractOptions, FrameRange, MediaFile, PixelFormat};

let config = ExtractOptions::new()
    .with_pixel_format(PixelFormat::Rgba8)
    .with_resolution(Some(1280), Some(720));

let mut unbundler = MediaFile::open("input.mp4")?;
let frames = unbundler.video().frames_with_options(
    FrameRange::Interval(30),
    &config,
)?;
```

### Read Metadata

```rust
use unbundle::MediaFile;

let unbundler = MediaFile::open("input.mp4")?;
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
use unbundle::MediaFile;

let unbundler = MediaFile::open("input.mp4")?;
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
use unbundle::MediaFile;

let unbundler = MediaFile::open("input.mkv")?;
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
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("input.mp4")?;

// Get a frame with its decode metadata
let (image, info) = unbundler.video().frame_and_metadata(0)?;
println!("Frame {}: keyframe={}, type={:?}, pts={:?}",
    info.frame_number, info.is_keyframe, info.frame_type, info.pts);
```

### Thumbnail Generation

```rust
use std::time::Duration;
use unbundle::{MediaFile, ThumbnailHandle, ThumbnailOptions};

let mut unbundler = MediaFile::open("input.mp4")?;

// Single thumbnail at a timestamp
let thumb = ThumbnailHandle::at_timestamp(&mut unbundler, Duration::from_secs(5), 320)?;

// Contact-sheet grid
let config = ThumbnailOptions::new(4, 3); // 4 columns × 3 rows
let grid = ThumbnailHandle::grid(&mut unbundler, &config)?;
grid.save("contact_sheet.png")?;

// Smart thumbnail (picks frame with highest visual variance)
let smart = ThumbnailHandle::smart(&mut unbundler, 10, 320)?;
```

### GIF Export

```rust
use std::time::Duration;
use unbundle::{FrameRange, GifOptions, MediaFile};

let mut unbundler = MediaFile::open("input.mp4")?;

let config = GifOptions::new().width(320).frame_delay(10);
unbundler.video().export_gif(
    "output.gif",
    FrameRange::TimeRange(Duration::from_secs(0), Duration::from_secs(5)),
    &config,
)?;

// Or export to memory
let bytes = unbundler.video().export_gif_to_memory(
    FrameRange::Interval(10),
    &config,
)?;
```

### Audio Waveform

```rust
use unbundle::{MediaFile, WaveformOptions};

let mut unbundler = MediaFile::open("input.mp4")?;
let waveform = unbundler.audio().generate_waveform(
    &WaveformOptions::new().bins(1000),
)?;

for bin in &waveform.bins {
    println!("min={:.3} max={:.3} rms={:.3}", bin.min, bin.max, bin.rms);
}
```

### Loudness Analysis

```rust
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("input.mp4")?;
let loudness = unbundler.audio().analyze_loudness()?;
println!("Peak: {:.1} dBFS, RMS: {:.1} dBFS", loudness.peak_dbfs, loudness.rms_dbfs);
```

### Audio Sample Iteration

```rust
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("input.mp4")?;
let iter = unbundler.audio().sample_iter()?;
let mut total_samples = 0u64;
for chunk in iter {
    let chunk = chunk?;
    total_samples += chunk.samples.len() as u64;
}
println!("Total mono samples: {total_samples}");
```

### Audio Transcoding

```rust
use unbundle::{AudioFormat, MediaFile, Transcoder};

let mut unbundler = MediaFile::open("input.mp4")?;

// Re-encode audio from the source format to MP3
Transcoder::new(&mut unbundler)
    .format(AudioFormat::Mp3)
    .run("output.mp3")?;
```

### Video Writing

```rust
use unbundle::{MediaFile, FrameRange, VideoEncoder, VideoEncoderOptions, VideoCodec};

let mut unbundler = MediaFile::open("input.mp4")?;
let frames = unbundler.video().frames(FrameRange::Interval(30))?;

let config = VideoEncoderOptions::default()
    .resolution(1920, 1080)
    .frames_per_second(24)
    .codec(VideoCodec::H264);
VideoEncoder::new(config).write("output.mp4", &frames)?;
```

### Keyframe & Group of Pictures Analysis

```rust
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("input.mp4")?;
let group_of_pictures = unbundler.video().analyze_group_of_pictures()?;
println!(
    "Keyframes: {}, Average Group of Pictures size: {:.1}",
    group_of_pictures.keyframes.len(),
    group_of_pictures.average_group_of_pictures_size
);
```

### VFR Detection

```rust
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("input.mp4")?;
let analysis = unbundler.video().analyze_variable_framerate()?;
println!("VFR: {}, mean FPS: {:.2}", analysis.is_variable_frame_rate, analysis.mean_frames_per_second);
```

### Packet Inspection

```rust
use unbundle::MediaFile;

let mut unbundler = MediaFile::open("input.mp4")?;
for packet in unbundler.packet_iter()? {
    let packet = packet?;
    println!("stream={} pts={:?} size={} key={}",
        packet.stream_index, packet.pts, packet.size, packet.is_keyframe);
}
```

## API Documentation

Complete API documentation is available at [docs.rs/unbundle](https://docs.rs/unbundle).

### Essential Types

| Type | Description |
|------|-------------|
| `MediaFile` | Main entry point — opens a media file and provides access to media handles |
| `VideoHandle` | Extracts video frames as `DynamicImage` |
| `AudioHandle` | Extracts audio tracks as bytes or files |
| `SubtitleHandle` | Extracts text-based subtitle tracks |
| `FrameRange` | Specifies which frames to extract (range, interval, timestamps, etc.) |
| `ExtractOptions` | Configure threading, progress callbacks, cancellation, pixel format, resolution, hardware acceleration |

### Stream & Iteration Types

| Type | Description |
|------|-------------|
| `FrameIterator` | Lazy, pull-based frame iterator |
| `AudioIterator` | Lazy pull-based audio sample iterator (mono f32) |
| `AudioChunk` | A chunk of decoded audio samples with timing |
| `PacketIterator` | Lazy raw-packet-level demuxer iterator |
| `FrameStream` | Async stream of decoded frames via Tokio (feature: `async`) |
| `AudioFuture` | Async audio extraction future (feature: `async`) |

### Configuration Types

| Type | Description |
|------|-------------|
| `FrameOutputOptions` | Pixel format and resolution settings for frame output |
| `PixelFormat` | Output pixel format (RGB8, RGBA8, GRAY8) |
| `AudioFormat` | Output audio format (WAV, MP3, FLAC, AAC) |
| `SubtitleFormat` | Output subtitle format (SRT, WebVTT, Raw) |
| `ThumbnailOptions` | Grid thumbnail options (columns, rows, width) |
| `GifOptions` | Animated GIF export configuration (width, delay, repeat) (feature: `gif`) |
| `WaveformOptions` | Waveform generation settings (bin count, time range) (feature: `waveform`) |
| `SceneDetectionOptions` | Scene detection threshold configuration (feature: `scene`) |
| `VideoEncoderOptions` | Video encoder settings (FPS, resolution, codec, CRF) (feature: `encode`) |
| `HardwareAccelerationMode` | Hardware acceleration mode selection (feature: `hardware`) |

### Metadata Types

| Type | Description |
|------|-------------|
| `MediaMetadata` | Container-level metadata (duration, format) |
| `VideoMetadata` | Video stream metadata (dimensions, frame rate, codec) |
| `AudioMetadata` | Audio stream metadata (sample rate, channels, codec) |
| `SubtitleMetadata` | Subtitle stream metadata (codec, language) |
| `ChapterMetadata` | Chapter information (title, start/end times) |
| `FrameMetadata` | Per-frame decode metadata (PTS, keyframe flag, picture type) |
| `FrameType` | Picture type enum (I, P, B, etc.) |
| `KeyFrameMetadata` | Keyframe position metadata (packet number, PTS, timestamp) |
| `GroupOfPicturesInfo` | Group of Pictures structure analysis result (keyframes, sizes, statistics) |
| `VariableFrameRateAnalysis` | Variable frame rate detection result (min/max/mean FPS) |
| `PacketInfo` | Per-packet metadata (stream index, PTS, DTS, size, keyframe) |

### Utility Types

| Type | Description |
|------|-------------|
| `MediaProbe` | Lightweight stateless media file probing |
| `ThumbnailHandle` | Thumbnail generation helpers (single, grid, smart) |
| `Remuxer` | Lossless container format conversion |
| `ValidationReport` | Result of media file validation |
| `ProgressCallback` | Trait for receiving progress updates |
| `ProgressInfo` | Progress event data (current, total, percentage, ETA) |
| `CancellationToken` | Cooperative cancellation via `Arc<AtomicBool>` |
| `OperationType` | Identifies the operation being tracked |
| `UnbundleError` | Error type with rich context |
| `SubtitleEvent` | A single decoded subtitle event (text, start/end time) |
| `BitmapSubtitleEvent` | A bitmap subtitle event with image and timing |

### Feature-Specific Types

| Type | Feature | Description |
|------|---------|-------------|
| `Transcoder` | `transcode` | Audio re-encoding builder (format, range, bitrate) |
| `VideoEncoder` | `encode` | Encodes image sequences into video files |
| `VideoCodec` | `encode` | Supported video codecs (H.264, H.265, MPEG-4) |
| `WaveformData` | `waveform` | Generated waveform result with per-bin statistics |
| `WaveformBin` | `waveform` | Single waveform bin (min, max, RMS amplitude) |
| `LoudnessInfo` | `loudness` | Peak/RMS loudness with dBFS equivalents |
| `SceneChange` | `scene` | Detected scene change with timestamp and score |
| `HardwareDeviceType` | `hardware` | Supported hardware device types (CUDA, VAAPI, etc.) |

## Performance

`unbundle` is designed for efficiency in both single-file and batch processing scenarios:

- **Smart seeking** — Uses FFmpeg's keyframe-based seeking. For sequential access (ranges, intervals), frames are decoded without redundant seeks.
- **Lightweight decoders** — Each extraction call creates a fresh decoder. FFmpeg decoder creation is fast relative to actual decoding work.
- **Batch optimization** — `FrameRange::Specific` sorts requested frame numbers and processes them in order to minimize seeks.
- **Memory-efficient streaming** — `for_each_frame` and `FrameIterator` process frames one at a time without buffering entire frame sets.
- **Parallel extraction** — `frames_parallel()` (feature `rayon`) splits frames across rayon threads, each with its own demuxer for true parallelism.
- **Hardware acceleration** — When enabled (feature `hardware`), attempts GPU-accelerated decoding with automatic fallback to software.
- **Correct stride handling** — Properly handles FFmpeg's row padding when converting frames to `image` buffers.
- **Zero-copy audio** — Uses `avio_open_dyn_buf` for in-memory audio encoding without temporary files.

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

### Test Coverage

The test suite includes comprehensive coverage:

| Test Module | Coverage |
|-------------|----------|
| `video` | Single frames, ranges, intervals, timestamps, specific lists, pixel formats, resolution scaling |
| `audio` | WAV/MP3/FLAC/AAC extraction, ranges, file output, multi-track |
| `subtitle` | Subtitle decoding, SRT/WebVTT export, multi-track |
| `metadata` | Container metadata, video/audio/subtitle stream properties |
| `configuration` | ExtractOptions builder, pixel formats, resolution, cancellation |
| `progress` | ProgressCallback, ProgressInfo fields, CancellationToken |
| `error_handling` | Error variants, context, invalid inputs, missing streams |
| `video_iterator` | FrameIterator, lazy iteration, early exit |
| `conversion` | Remuxer, stream exclusion, lossless format conversion |
| `validation` | ValidationReport, warnings, errors, valid files |
| `chapters` | Chapter metadata extraction, titles, timestamps, ordering |
| `frame_metadata` | FrameMetadata, FrameType, keyframe detection, PTS values |
| `segmented_extraction` | FrameRange::Segments, multiple disjoint time ranges |
| `probing` | MediaProbe, probe/probe_many, error handling |
| `thumbnail` | ThumbnailHandle, grid, smart selection, aspect ratio |
| `audio_iterator` | AudioIterator, chunk iteration, sample rates |
| `keyframe` | GroupOfPicturesInfo, KeyFrameMetadata, Group of Pictures statistics |
| `variable_framerate` | VariableFrameRateAnalysis, constant vs variable frame rate |
| `packet_iterator` | PacketIterator, PacketInfo, stream filtering |
| `subtitle_search` | Subtitle search, case-insensitive matching |
| `metadata_extended` | Extended metadata: video tracks, colorspace, HDR |

Feature-specific tests (require corresponding features enabled):

| Test Module | Feature Required | Coverage |
|-------------|------------------|----------|
| `scene` | `scene` | Scene change detection, threshold configuration |
| `async_extraction` | `async` | FrameStream, AudioFuture, async streaming |
| `rayon` | `rayon` | frames_parallel, sequential parity, interval mode |
| `hardware_acceleration` | `hardware` | Hardware device enumeration, Auto/Software modes |
| `gif_export` | `gif` | GIF encoding, file and in-memory output |
| `waveform` | `waveform` | WaveformOptions, bin statistics, time ranges |
| `loudness` | `loudness` | Peak/RMS loudness, dBFS values |
| `video_encoder` | `encode` | VideoEncoder, codec selection, frame encoding |
| `transcode` | `transcode` | Transcoder, format conversion, time ranges |

## Benchmarks

Run performance benchmarks:

```bash
cargo bench --all-features
```

Criterion benchmarks are located in `benches/` and measure:
- Frame extraction throughput (single vs parallel)
- Seek performance (sequential vs random access)
- Audio extraction speed across formats
- Iterator overhead vs batch extraction

## Troubleshooting

### FFmpeg Linking Errors

**Problem:** `error: linking with 'cc' failed` or `cannot find -lavcodec`

**Solution:**
- Ensure FFmpeg development libraries are installed (see Installation)
- Set `PKG_CONFIG_PATH` to point to FFmpeg's `.pc` files
- On Windows, set `FFMPEG_DIR` environment variable
- Verify with: `pkg-config --libs --cflags libavcodec`

### Codec Not Supported

**Problem:** `UnbundleError::UnsupportedAudioFormat`

**Solution:**
- Check that your FFmpeg build includes the required codec
- Run `ffmpeg -codecs` to list available codecs
- Some codecs require FFmpeg to be built with specific flags (e.g., `--enable-libx264`)

### Hardware Acceleration Fails

**Problem:** Hardware decoding falls back to software or fails entirely

**Solution:**
- Verify GPU drivers are up to date
- Check available hardware devices: `unbundle::hardware_acceleration::available_hardware_devices()`
- Use `ExtractOptions::with_hardware_acceleration(HardwareAccelerationMode::Auto)` for automatic fallback
- Not all codecs/formats support hardware acceleration
- Try `ffmpeg -hwaccels` to list available hardware acceleration methods

### Out of Memory

**Problem:** High memory usage when extracting many frames

**Solution:**
- Use streaming iteration instead of batch extraction: `frame_iter()` or `for_each_frame()`
- Process frames in smaller batches
- Use `AudioIterator` for large audio files instead of loading entire tracks

### Slow Frame Extraction

**Problem:** Frame extraction is slower than expected

**Solution:**
- Use `frames_parallel()` (feature `rayon`) for CPU-bound workloads
- Enable hardware acceleration (feature `hardware`) for high-resolution video
- Avoid extracting specific frames in random order — sorted access is much faster
- Consider using `FrameRange::Interval` instead of many individual frame numbers

### Permission Denied / File Not Found

**Problem:** Cannot open media file

**Solution:**
- Verify file path is correct and file exists
- Check file permissions (readable by current user)
- Ensure file is not locked by another process
- On Windows, use raw string literals for paths: `r"C:\path\to\video.mp4"`

## Contributing

Contributions are welcome! Please see the [GitHub repository](https://github.com/skanderjeddi/unbundle) for:
- Bug reports and feature requests
- Pull requests
- Discussions and questions

## License

MIT — see [LICENSE](LICENSE) file for details.
