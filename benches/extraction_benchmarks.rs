//! Benchmarks for frame and audio extraction.
//!
//! Run with: cargo bench
//!
//! Requires fixture files from `tests/fixtures/generate_fixtures.sh`.

use std::{path::Path, time::Duration};

use criterion::Criterion;
use unbundle::{AudioFormat, FrameRange, MediaUnbundler};

const SAMPLE_VIDEO: &str = "tests/fixtures/sample_video.mp4";

fn benchmark_single_frame_extraction(criterion: &mut Criterion) {
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

criterion::criterion_group!(
    benches,
    benchmark_single_frame_extraction,
    benchmark_frame_range_extraction,
    benchmark_audio_extraction,
);
criterion::criterion_main!(benches);
