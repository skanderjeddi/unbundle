# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to Semantic Versioning.

## [4.3.7] - 2026-02-14

### Added
- Added a first-party CLI MVP as `unbundle-cli` with common commands:
  - `metadata`
  - `frame`
  - `audio`
  - `subtitle`
- Added a best-effort Windows build helper (`build.rs`) that detects vcpkg-style FFmpeg installs and emits actionable setup warnings.
- Added this changelog.

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

## [4.3.3] - 2026-02-14

### Added
- Added stream-copy progress callback reporting for video/audio/subtitle operations.
- Added advanced filter-chain coverage in tests.

## [4.3.2] - 2026-02-14

### Added
- Added custom FFmpeg filter graph frame extraction APIs.

## [4.3.1] - 2026-02-14

### Added
- Added raw stream-copy support for video and expanded stream-copy test coverage across media types.
