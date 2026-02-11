# unbundle

Unbundle media files — extract still frames and audio from video files.

`unbundle` provides a clean, ergonomic Rust API for extracting video frames as
[`image::DynamicImage`](https://docs.rs/image/latest/image/enum.DynamicImage.html)
values and audio tracks as encoded byte vectors, powered by FFmpeg via
[`ffmpeg-next`](https://crates.io/crates/ffmpeg-next).

## Features

- **Frame extraction** — by frame number, timestamp, range, interval, or
  specific frame list
- **Audio extraction** — to WAV, MP3, FLAC, or AAC
- **In-memory or file-based** audio output
- **Rich metadata** — video dimensions, frame rate, frame count, audio sample
  rate, channels, codec, and more
- **Efficient seeking** — seeks to the nearest keyframe, then decodes forward
- **Zero-copy in-memory audio** — uses FFmpeg's dynamic buffer I/O to encode
  audio directly to memory without temporary files

## Installation

Add `unbundle` to your `Cargo.toml`:

```toml
[dependencies]
unbundle = "0.1"
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
use unbundle::MediaUnbundler;

let mut unbundler = MediaUnbundler::open("input.mp4")?;

// Extract the first frame
let frame = unbundler.video().frame(0)?;
frame.save("first_frame.png")?;

// Extract a frame at 30 seconds
use std::time::Duration;
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
```

## API Documentation

See the [API docs](https://docs.rs/unbundle) for complete documentation.

### Core Types

| Type | Description |
|------|-------------|
| `MediaUnbundler` | Main entry point — opens a media file and provides access to extractors |
| `VideoExtractor` | Extracts video frames as `DynamicImage` |
| `AudioExtractor` | Extracts audio tracks as bytes or files |
| `FrameRange` | Specifies which frames to extract (range, interval, timestamps, etc.) |
| `AudioFormat` | Output audio format (WAV, MP3, FLAC, AAC) |
| `MediaMetadata` | Container-level metadata (duration, format) |
| `VideoMetadata` | Video stream metadata (dimensions, frame rate, codec) |
| `AudioMetadata` | Audio stream metadata (sample rate, channels, codec) |
| `UnbundleError` | Error type with rich context |

## Examples

See the [`examples/`](examples/) directory:

- **`extract_frames`** — Extract video frames by number, timestamp, and interval
- **`extract_audio`** — Extract the complete audio track
- **`extract_audio_segment`** — Extract a specific time range as MP3
- **`thumbnail_grid`** — Create a thumbnail grid from evenly-spaced frames
- **`metadata`** — Display all media metadata

Run an example:

```bash
cargo run --example metadata -- path/to/video.mp4
```

## Performance

- **Seeking:** Uses FFmpeg's keyframe-based seeking. For sequential access
  (ranges, intervals), frames are decoded without redundant seeks.
- **Decoder reuse:** Each extraction call creates a fresh, lightweight decoder.
  FFmpeg decoder creation is fast relative to actual decoding.
- **Batch optimisation:** `FrameRange::Specific` sorts requested frame numbers
  and processes them in order to minimise seeks.
- **Stride handling:** Correctly handles FFmpeg's row padding when converting
  frames to `image::RgbImage`.
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
cargo test
```

## License

MIT
