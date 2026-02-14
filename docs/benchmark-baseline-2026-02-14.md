# Benchmark Baseline — 2026-02-14 (Windows local)

This baseline was captured on a local Windows development machine using Criterion.

## Run context

- Crate: `unbundle` v5.0.0
- Bench target: `unbundle_benchmarks`
- OS: Windows (local)
- FFmpeg runtime: vcpkg dynamic libraries

### Local environment used for stable runtime linking

```powershell
$env:FFMPEG_DIR='C:\Users\skand\vcpkg\installed\x64-windows'
$env:VCPKGRS_DYNAMIC='1'
$env:PATH='C:\Users\skand\vcpkg\installed\x64-windows\bin;' + $env:PATH
```

## Commands executed

### Default benchmark sweep

```powershell
cargo bench --bench unbundle_benchmarks -- --noplot *>&1 | Tee-Object -FilePath target/bench-default.log
```

### All-features benchmark sweep (partial, see notes)

```powershell
cargo bench --bench unbundle_benchmarks --all-features -- --noplot *>&1 | Tee-Object -FilePath target/bench-all-features.log
```

### Remaining failed group rerun (post-fix)

```powershell
cargo bench --bench unbundle_benchmarks --all-features -- feature_encode --noplot
```

## Key default timings (median range)

| Benchmark | Time |
|---|---:|
| `core/open_probe_validate/open` | `[2.9786 ms 3.1098 ms 3.2670 ms]` |
| `video/single_frame/frame/0` | `[4.6509 ms 4.7524 ms 4.8655 ms]` |
| `video/single_frame/frame/120` | `[25.600 ms 26.103 ms 26.625 ms]` |
| `video/single_frame/frame_with_filter` | `[19.916 ms 20.354 ms 20.884 ms]` |
| `video/batch_modes/frames_range_0_29` | `[18.630 ms 18.872 ms 19.146 ms]` |
| `video/iterators/for_each_raw_frame_range` | `[7.4481 ms 7.5059 ms 7.5653 ms]` |
| `audio/extract/extract_full_memory/WAV` | `[9.5718 ms 10.355 ms 11.304 ms]` |
| `audio/extract/extract_full_memory/MP3` | `[44.576 ms 46.136 ms 47.910 ms]` |
| `audio/extract/extract_full_memory/AAC` | `[160.18 ms 162.20 ms 164.27 ms]` |
| `subtitle/extract` | `[2.6248 ms 2.6414 ms 2.6609 ms]` |
| `analysis/packet_keyframe_vfr/packet_iter` | `[3.0028 ms 3.0306 ms 3.0633 ms]` |
| `stream_copy_and_remux/video_stream_copy_to_memory_matroska` | `[11.493 ms 11.585 ms 11.714 ms]` |
| `thumbnail/thumbnail_grid_3x3` | `[97.927 ms 98.803 ms 99.682 ms]` |

Source: [target/bench-default.log](target/bench-default.log)

## Key all-features timings (captured before encode fix)

| Benchmark | Time |
|---|---:|
| `feature_scene/full_default` | `[93.882 ms 96.877 ms 100.29 ms]` |
| `feature_scene/keyframe_mode` | `[6.0688 ms 6.4452 ms 6.9823 ms]` |
| `feature_rayon/frames_parallel_range` | `[50.161 ms 51.577 ms 53.637 ms]` |
| `feature_async/frame_stream_range` | `[29.468 ms 31.025 ms 33.085 ms]` |
| `feature_async/extract_async_wav` | `[15.657 ms 15.994 ms 16.453 ms]` |
| `feature_hardware/software` | `[13.425 ms 13.493 ms 13.586 ms]` |
| `feature_hardware/auto` | `[13.382 ms 13.492 ms 13.610 ms]` |
| `feature_gif/gif_to_memory_short` | `[28.896 ms 29.044 ms 29.229 ms]` |
| `feature_waveform/generate_waveform/2000` | `[10.299 ms 10.362 ms 10.454 ms]` |
| `feature_loudness/analyze` | `[9.3562 ms 9.4517 ms 9.5642 ms]` |
| `feature_transcode/to_memory_mp3` | `[49.728 ms 51.538 ms 53.185 ms]` |

Source: [target/bench-all-features.log](target/bench-all-features.log)

## Encode benchmark recovery (post-fix)

- Previous all-features run failed at:
  - `feature_encode/encode_short_clip_h264`
  - error: `h264_mf ... could not set output type` / `VideoEncodeError`
- Benchmark updated to use `VideoCodec::Mpeg4` for Windows portability.
- Isolated rerun result:
  - `feature_encode/encode_short_clip_mpeg4`
  - time: `[5.4487 ms 5.5842 ms 5.7337 ms]`

Source: [target/bench-feature-encode.log](target/bench-feature-encode.log)

## Notes and caveats

- Criterion emitted multiple "Unable to complete ... in 5.0s" warnings for slower groups; this is expected with current per-group sample sizes.
- Some comparisons in logs include prior baseline deltas from earlier local runs.
- For publishing externally, rerun once in a quiet environment and keep this env setup fixed to reduce variance.

## Recommended next pass (optional)

- Run one clean all-features sweep after the encode fix to produce a single uninterrupted all-features log.
- Add a Linux baseline to compare against Windows media stack behavior.
- Export machine-readable summaries (CSV/JSON) for plotting trend lines in CI.
