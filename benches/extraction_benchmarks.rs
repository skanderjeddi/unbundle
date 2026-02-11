//! Benchmarks for frame, audio, and other extraction operations.
//!
//! Run with: cargo bench
//! Run with all features: cargo bench --all-features
//!
//! Requires fixture files from `tests/fixtures/generate_fixtures.sh`.

use std::{path::Path, time::Duration};

use criterion::Criterion;
use ffmpeg_next::util::log::Level as LogLevel;
use unbundle::{
    AudioFormat, ExtractionConfig, FrameRange, MediaUnbundler,
    PixelFormat, Remuxer,
};

#[cfg(feature = "hw-accel")]
use unbundle::HwAccelMode;
#[cfg(feature = "scene-detection")]
use unbundle::SceneDetectionConfig;

#[cfg(feature = "async-tokio")]
use tokio::runtime::Runtime;

const SAMPLE_VIDEO: &str = "tests/fixtures/sample_video.mp4";
const SAMPLE_MKV: &str = "tests/fixtures/sample_video.mkv";
const SAMPLE_WITH_SUBS: &str = "tests/fixtures/sample_with_subtitles.mkv";

fn benchmark_single_frame_extraction(criterion: &mut Criterion) {
    ffmpeg_next::util::log::set_level(LogLevel::Error);

    if !Path::new(SAMPLE_VIDEO).exists() {
        eprintln!("Skipping benchmark: fixture not found");
        return;
    }

    criterion.bench_function("extract single frame (sequential)", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let _frame = unbundler.video().frame(0).unwrap();
        });
    });

    criterion.bench_function("extract single frame (mid-video seek)", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let _frame = unbundler.video().frame(75).unwrap();
        });
    });
}

fn benchmark_frame_range_extraction(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_VIDEO).exists() {
        return;
    }

    criterion.bench_function("extract 10 consecutive frames", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let _frames = unbundler.video().frames(FrameRange::Range(0, 9)).unwrap();
        });
    });
}

fn benchmark_for_each_frame(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_VIDEO).exists() {
        return;
    }

    criterion.bench_function("for_each_frame 10 frames", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            unbundler
                .video()
                .for_each_frame(FrameRange::Range(0, 9), |_, _| Ok(()))
                .unwrap();
        });
    });
}

fn benchmark_frame_iterator(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_VIDEO).exists() {
        return;
    }

    criterion.bench_function("frame_iter 10 frames", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let iter = unbundler.video().frame_iter(FrameRange::Range(0, 9)).unwrap();
            for result in iter {
                let _ = result.unwrap();
            }
        });
    });
}

fn benchmark_pixel_formats(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_VIDEO).exists() {
        return;
    }

    criterion.bench_function("extract frame RGBA8", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let config = ExtractionConfig::new()
                .with_pixel_format(PixelFormat::Rgba8);
            let _frames = unbundler
                .video()
                .frames_with_config(FrameRange::Range(0, 0), &config)
                .unwrap();
        });
    });

    criterion.bench_function("extract frame Gray8", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let config = ExtractionConfig::new()
                .with_pixel_format(PixelFormat::Gray8);
            let _frames = unbundler
                .video()
                .frames_with_config(FrameRange::Range(0, 0), &config)
                .unwrap();
        });
    });

    criterion.bench_function("extract frame scaled 320w", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let config = ExtractionConfig::new()
                .with_resolution(Some(320), None);
            let _frames = unbundler
                .video()
                .frames_with_config(FrameRange::Range(0, 0), &config)
                .unwrap();
        });
    });
}

fn benchmark_audio_extraction(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_VIDEO).exists() {
        return;
    }

    criterion.bench_function("extract full audio (WAV, to memory)", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let _audio = unbundler.audio().extract(AudioFormat::Wav).unwrap();
        });
    });

    criterion.bench_function("extract audio range (WAV, 1s-3s)", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let _audio = unbundler
                .audio()
                .extract_range(
                    Duration::from_secs(1),
                    Duration::from_secs(3),
                    AudioFormat::Wav,
                )
                .unwrap();
        });
    });
}

fn benchmark_validation(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_VIDEO).exists() {
        return;
    }

    criterion.bench_function("validate media file", |bencher| {
        bencher.iter(|| {
            let unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let _report = unbundler.validate();
        });
    });
}

fn benchmark_remuxing(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_MKV).exists() {
        return;
    }

    criterion.bench_function("remux MKV to MP4", |bencher| {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let output_path = tmp.path().with_extension("mp4");
        bencher.iter(|| {
            Remuxer::new(SAMPLE_MKV, &output_path).unwrap().run().unwrap();
        });
        let _ = std::fs::remove_file(&output_path);
    });
}

fn benchmark_subtitle_extraction(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_WITH_SUBS).exists() {
        return;
    }

    criterion.bench_function("extract subtitle entries", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_WITH_SUBS).unwrap();
            let _entries = unbundler.subtitle().extract().unwrap();
        });
    });
}

#[cfg(feature = "scene-detection")]
fn benchmark_scene_detection(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_VIDEO).exists() {
        return;
    }

    let mut group = criterion.benchmark_group("scene detection");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("default threshold", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let _scenes = unbundler.video().detect_scenes(None).unwrap();
        });
    });

    group.bench_function("low threshold", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let config = SceneDetectionConfig { threshold: 1.0 };
            let _scenes = unbundler.video().detect_scenes(Some(config)).unwrap();
        });
    });

    group.finish();
}

#[cfg(not(feature = "scene-detection"))]
fn benchmark_scene_detection(_criterion: &mut Criterion) {}

#[cfg(feature = "hw-accel")]
fn benchmark_hwaccel(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_VIDEO).exists() {
        return;
    }

    let mut group = criterion.benchmark_group("hw-accel");
    group.sample_size(30);

    group.bench_function("auto", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let config = ExtractionConfig::new()
                .with_hw_accel(HwAccelMode::Auto);
            let _frames = unbundler
                .video()
                .frames_with_config(FrameRange::Range(0, 0), &config)
                .unwrap();
        });
    });

    group.bench_function("software fallback", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let config = ExtractionConfig::new()
                .with_hw_accel(HwAccelMode::Software);
            let _frames = unbundler
                .video()
                .frames_with_config(FrameRange::Range(0, 0), &config)
                .unwrap();
        });
    });

    group.finish();
}

#[cfg(not(feature = "hw-accel"))]
fn benchmark_hwaccel(_criterion: &mut Criterion) {}

#[cfg(feature = "parallel")]
fn benchmark_parallel(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_VIDEO).exists() {
        return;
    }

    let mut group = criterion.benchmark_group("parallel");
    group.sample_size(30);

    group.bench_function("10 frames", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let config = ExtractionConfig::new();
            let _frames = unbundler
                .video()
                .frames_parallel(FrameRange::Range(0, 9), &config)
                .unwrap();
        });
    });

    group.bench_function("specific frames", |bencher| {
        bencher.iter(|| {
            let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
            let config = ExtractionConfig::new();
            let _frames = unbundler
                .video()
                .frames_parallel(
                    FrameRange::Specific(vec![0, 30, 60, 90, 120]),
                    &config,
                )
                .unwrap();
        });
    });

    group.finish();
}

#[cfg(not(feature = "parallel"))]
fn benchmark_parallel(_criterion: &mut Criterion) {}

#[cfg(feature = "async-tokio")]
fn benchmark_async(criterion: &mut Criterion) {
    if !Path::new(SAMPLE_VIDEO).exists() {
        return;
    }

    let rt = Runtime::new().unwrap();
    let mut group = criterion.benchmark_group("async");
    group.sample_size(30);

    group.bench_function("frame_stream 10 frames", |bencher| {
        bencher.iter(|| {
            rt.block_on(async {
                let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
                let config = ExtractionConfig::new();
                let mut stream = unbundler
                    .video()
                    .frame_stream(FrameRange::Range(0, 9), config)
                    .unwrap();

                use tokio_stream::StreamExt;
                while let Some(result) = stream.next().await {
                    let _ = result.unwrap();
                }
            });
        });
    });

    group.bench_function("extract_audio", |bencher| {
        bencher.iter(|| {
            rt.block_on(async {
                let mut unbundler = MediaUnbundler::open(SAMPLE_VIDEO).unwrap();
                let config = ExtractionConfig::new();
                let _audio = unbundler
                    .audio()
                    .extract_async(AudioFormat::Wav, config)
                    .unwrap()
                    .await
                    .unwrap();
            });
        });
    });

    group.finish();
}

#[cfg(not(feature = "async-tokio"))]
fn benchmark_async(_criterion: &mut Criterion) {}

criterion::criterion_group!(
    benches,
    benchmark_single_frame_extraction,
    benchmark_frame_range_extraction,
    benchmark_for_each_frame,
    benchmark_frame_iterator,
    benchmark_pixel_formats,
    benchmark_audio_extraction,
    benchmark_validation,
    benchmark_remuxing,
    benchmark_subtitle_extraction,
    benchmark_scene_detection,
    benchmark_hwaccel,
    benchmark_parallel,
    benchmark_async,
);
criterion::criterion_main!(benches);
