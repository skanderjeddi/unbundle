# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to Semantic Versioning.

## [5.0.0] - 2026-02-14

### Added
- Added `MediaFile::probe_only()` for lightweight metadata probing without retaining an open demuxer.
- Added `FrameRange::KeyframesOnly` to extract keyframes directly.
- Added zero-copy raw frame callback API:
  - `for_each_raw_frame`
  - `for_each_raw_frame_with_options`
- Added bitmap subtitle rendering helpers:
  - `SubtitleHandle::render_at(...)`
  - `BitmapSubtitleEvent::as_image()`

### Changed
- Consolidated raw-frame callback APIs on crate-owned `RawFrameView` callbacks.

### Removed
- Removed `for_each_frame_raw` and `for_each_frame_raw_with_options` in favor of `for_each_raw_frame` and `for_each_raw_frame_with_options`.

## [4.3.8] - 2026-02-14

### Added
- Added chainable FFmpeg filter API via `video().filter(...).filter(...).frame(...)`.
- Added raw frame callback APIs:
  - `for_each_frame_raw`
  - `for_each_frame_raw_with_options`
- Added URL/source-specific open error variant: `UnbundleError::SourceOpen`.
- Added integration coverage for chainable filters, raw frame callbacks, and URL-open errors.

### Improved
- Improved `open_url` and source-opening diagnostics by returning source-aware errors for URL-like inputs.
- Updated docs and README examples for chainable filters and URL-based CLI usage.

## [4.3.7] - 2026-02-14

### Added
- Added a first-party CLI MVP as `unbundle-cli` with common commands:
  - `metadata`
  - `frame`
  - `frame-at`
  - `audio`
  - `subtitle`
- Added a best-effort Windows build helper (`build.rs`) that detects vcpkg-style FFmpeg installs and emits actionable setup warnings.
- Added this changelog.
- Added video-encoder alias builder test coverage.

### Improved
- Added additional alias-coverage integration tests for builder-style APIs.

## [4.3.6] - 2026-02-14

### Added
- Added remaining fluent alias coverage tests for `Transcoder`, `VideoEncoderOptions`, `SceneDetectionOptions`, and `ThumbnailOptions`.

## [4.3.5] - 2026-02-14

### Added
- Added `open_url` example (`examples/open_url.rs`).
- Added URL/source-open coverage for async and rayon extraction paths.

### Improved
- Expanded README and crate docs with URL/source opening guidance.

## [4.3.4] - 2026-02-14

### Added
- Added source-based reopening support (`open_source`) used by async/rayon internals.
- Added `MediaFile::open_url`.
- Added `with_*` builder aliases across option/builder types.

### Improved
- Added integration coverage for source-opening and builder aliases.
- Added waveform alias-builder test coverage.

## [4.3.3] - 2026-02-14

### Added
- Added stream-copy progress callback reporting for video/audio/subtitle operations.
- Added advanced filter-chain coverage in tests.
- Added stream-copy test coverage and CI matrix/documentation updates.

## [4.3.2] - 2026-02-14

### Added
- Added custom FFmpeg filter graph frame extraction APIs.

### Fixed
- Fixed a `rayon.rs` typo as part of the release bump commit.

## [4.3.1] - 2026-02-14

### Added
- Added FFmpeg log-level configuration API (`FfmpegLogLevel`, `set_ffmpeg_log_level`, `get_ffmpeg_log_level`).

### Fixed
- Improved robustness of media open by skipping attached-picture streams and treating decoder creation failures as non-fatal per stream.

### Improved
- Applied comprehensive style and formatting audit.

## [4.2.2] - 2026-02-13

### Improved
- Cached decoder/scaler state in `VideoHandle::frame_with_options` for repeated single-frame extraction performance.

## [4.2.1] - 2026-02-12

### Added
- Added scene-detection modes for practical operation (`Auto`, `Keyframes`) and bounded analysis controls (`max_duration`, `max_scene_changes`).

### Fixed
- Fixed scene detection seek timestamps and normalized pixel format handling.
- Corrected `Pixel` → `AVPixelFormat` conversion.
- Included colorspace/color-range in buffer filter setup and corrected FFmpeg buffer filter parameter names.

### Improved
- Applied scene-module formatting cleanup.

## [4.0.0] - 2026-02-12

### Changed
- Refactored module/file naming to idiomatic conventions.
- Reorganized public API exports.

## [2.0.0] - 2026-02-12

### Added
- Added async/parallel examples and tests.

### Changed
- Restructured modules around expanded feature set.

## [Pre-2.0 Milestones] - 2026-02-11 to 2026-02-12

### Added
- Initial project with core video/audio extraction.
- `FrameIterator` for lazy pull-based frame decoding.
- `PacketWriter` abstraction for audio output pathways.
- Optional features: async extraction, scene detection, subtitles, hardware acceleration, and parallel extraction.
- MediaProbe, thumbnails, chapter metadata, frame metadata, and segmented extraction.
- GIF export, waveform/loudness analysis, transcoding, video encoding, keyframe/Group of Pictures analysis, variable frame rate analysis, and packet/audio iterators.

### Changed
- Removed binary fixtures from git and updated ignore rules.

[Unreleased]: https://github.com/skanderjeddi/unbundle/compare/v5.0.0...HEAD
[5.0.0]: https://github.com/skanderjeddi/unbundle/compare/v4.3.8...v5.0.0
[4.3.8]: https://github.com/skanderjeddi/unbundle/compare/v4.3.7...v4.3.8
[4.3.7]: https://github.com/skanderjeddi/unbundle/compare/v4.3.6...v4.3.7
[4.3.6]: https://github.com/skanderjeddi/unbundle/compare/v4.3.5...v4.3.6
[4.3.5]: https://github.com/skanderjeddi/unbundle/compare/v4.3.4...v4.3.5
[4.3.4]: https://github.com/skanderjeddi/unbundle/compare/v4.3.3...v4.3.4
[4.3.3]: https://github.com/skanderjeddi/unbundle/compare/v4.3.2...v4.3.3
[4.3.2]: https://github.com/skanderjeddi/unbundle/compare/v4.3.1...v4.3.2
[4.3.1]: https://github.com/skanderjeddi/unbundle/compare/v4.2.2...v4.3.1
[4.2.2]: https://github.com/skanderjeddi/unbundle/compare/v4.2.1...v4.2.2
[4.2.1]: https://github.com/skanderjeddi/unbundle/compare/v4.0.0...v4.2.1
[4.0.0]: https://github.com/skanderjeddi/unbundle/compare/v2.0.0...v4.0.0
[2.0.0]: https://github.com/skanderjeddi/unbundle/compare/eaafb1e...v2.0.0
