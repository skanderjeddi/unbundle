use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[cfg(feature = "waveform")]
use std::io::Write;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use serde_json::json;
use unbundle::{
    AudioFormat, ExtractOptions, FfmpegLogLevel, MediaFile, PixelFormat, ProgressCallback,
    ProgressInfo, SubtitleFormat,
};

#[cfg(feature = "hardware")]
use unbundle::{HardwareAccelerationMode, HardwareDeviceType};

#[cfg(feature = "loudness")]
use unbundle::LoudnessInfo;

#[cfg(feature = "scene")]
use unbundle::SceneDetectionOptions;

#[cfg(feature = "waveform")]
use unbundle::WaveformOptions;

const CLI_AFTER_HELP: &str = "Examples:\n  unbundle metadata input.mp4 --json\n  unbundle extract-frames input.mp4 --out frames --every 10 --progress --verbose\n  unbundle extract-audio input.mp4 --format mp3 --out audio.mp3\n  unbundle remux input.mkv output.mp4\n  unbundle completions zsh > _unbundle";

#[derive(Debug, Parser)]
#[command(
    name = "unbundle",
    version,
    about = "Extract frames, audio, subtitles, and metadata from media files",
    after_help = CLI_AFTER_HELP
)]
struct Cli {
    #[command(flatten)]
    global: GlobalOptions,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Parser, Clone, Default)]
struct GlobalOptions {
    /// Show additional logging output.
    #[arg(long)]
    verbose: bool,

    /// Show a progress bar where supported.
    #[arg(long)]
    progress: bool,

    /// Allow overwriting existing output files.
    #[arg(long)]
    overwrite: bool,

    /// FFmpeg log level (quiet, panic, fatal, error, warning, info, verbose, debug, trace).
    #[arg(long)]
    log_level: Option<String>,

    /// Preferred frame pixel format for extraction (rgb8, rgba8, gray8).
    #[arg(long)]
    pixel_format: Option<String>,

    /// Desired worker thread count for thread-aware commands.
    #[arg(long)]
    threads: Option<usize>,

    /// Hardware decode mode (auto, software, cuda, vaapi, dxva2, d3d11va, videotoolbox, qsv).
    #[arg(long)]
    hardware: Option<String>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Print metadata for a media file (alias: probe).
    #[command(
        about = "Print media metadata",
        visible_alias = "probe",
        visible_alias = "info",
        after_help = "Examples:\n  unbundle metadata input.mp4\n  unbundle metadata input.mp4 --json"
    )]
    Metadata {
        /// Input media path or URL.
        input: String,

        /// Output metadata as machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Extract frames to an output directory.
    #[command(
        about = "Extract video frames",
        after_help = "Examples:\n  unbundle extract-frames input.mp4 --out frames --every 10 --ext jpg\n  unbundle extract-frames input.mp4 --out frames --start 0:00:10 --end 0:00:20 --progress"
    )]
    ExtractFrames {
        /// Input media path or URL.
        input: String,
        /// Output directory for extracted frame images.
        #[arg(long)]
        out: PathBuf,
        /// Extract every Nth frame.
        #[arg(long, default_value_t = 30)]
        every: u64,
        /// Optional start frame (inclusive).
        #[arg(long)]
        start: Option<String>,
        /// Optional end frame (inclusive).
        #[arg(long)]
        end: Option<String>,
        /// Output image extension (png, jpg, jpeg, bmp, tiff).
        #[arg(long, default_value = "png")]
        ext: String,
    },

    /// Extract audio track to a file.
    #[command(
        about = "Extract audio track",
        after_help = "Examples:\n  unbundle extract-audio input.mp4 --format mp3 --out audio.mp3\n  unbundle extract-audio input.mp4 --format wav --out clip.wav --start 00:01:00 --end 00:01:30"
    )]
    ExtractAudio {
        /// Input media path or URL.
        input: String,
        /// Output format: wav | mp3 | flac | aac.
        #[arg(long)]
        format: String,
        /// Output file path.
        #[arg(long)]
        out: PathBuf,
        /// Optional start time in seconds.
        #[arg(long)]
        start: Option<String>,
        /// Optional end time in seconds.
        #[arg(long)]
        end: Option<String>,
    },

    /// Extract subtitles to a file.
    #[command(
        about = "Extract subtitle track",
        after_help = "Examples:\n  unbundle extract-subs input.mkv --format srt --out subs.srt\n  unbundle extract-subs input.mkv --format raw --out lines.txt --start 00:00:10 --end 00:00:40"
    )]
    ExtractSubs {
        /// Input media path or URL.
        input: String,
        /// Output format: srt | vtt | raw.
        #[arg(long)]
        format: String,
        /// Output file path.
        #[arg(long)]
        out: PathBuf,
        /// Optional start time in seconds.
        #[arg(long)]
        start: Option<String>,
        /// Optional end time in seconds.
        #[arg(long)]
        end: Option<String>,
    },

    /// Generate thumbnails from video.
    #[command(
        about = "Generate thumbnails",
        after_help = "Examples:\n  unbundle thumbnail input.mp4 --out thumb.jpg --mode single --timestamp 00:00:10\n  unbundle thumbnail input.mp4 --out grid.jpg --mode grid --columns 4 --rows 3"
    )]
    Thumbnail {
        input: String,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value = "single")]
        mode: String,
        #[arg(long)]
        timestamp: Option<String>,
        #[arg(long)]
        frame: Option<u64>,
        #[arg(long, default_value_t = 4)]
        columns: u32,
        #[arg(long, default_value_t = 3)]
        rows: u32,
        #[arg(long, default_value_t = 10)]
        samples: u32,
        #[arg(long, default_value_t = 640)]
        max_dimension: u32,
    },

    /// Losslessly remux container format (e.g. MKV -> MP4).
    #[command(about = "Remux container without re-encoding")]
    Remux {
        input: String,
        output: PathBuf,
        #[arg(long)]
        exclude_video: bool,
        #[arg(long)]
        exclude_audio: bool,
        #[arg(long)]
        exclude_subtitles: bool,
    },

    /// Validate media structure and print a report.
    #[command(
        about = "Validate media file",
        after_help = "Examples:\n  unbundle validate input.mp4"
    )]
    Validate {
        /// Input media path or URL.
        input: String,
    },

    #[cfg(feature = "scene")]
    /// Detect scene changes and print timestamps.
    #[command(about = "Detect scene changes")]
    SceneDetect {
        input: String,
        #[arg(long, default_value_t = 10.0)]
        threshold: f64,
        #[arg(long)]
        json: bool,
    },

    #[cfg(feature = "waveform")]
    /// Analyze waveform and print summary or write CSV.
    #[command(about = "Analyze audio waveform")]
    Waveform {
        input: String,
        #[arg(long, default_value_t = 800)]
        bins: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    #[cfg(feature = "loudness")]
    /// Analyze audio loudness.
    #[command(about = "Analyze audio loudness")]
    Loudness {
        input: String,
        #[arg(long)]
        json: bool,
    },

    /// Generate shell completion scripts.
    #[command(about = "Generate shell completions")]
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

fn parse_audio_format(value: &str) -> Option<AudioFormat> {
    match value.to_ascii_lowercase().as_str() {
        "wav" => Some(AudioFormat::Wav),
        "mp3" => Some(AudioFormat::Mp3),
        "flac" => Some(AudioFormat::Flac),
        "aac" => Some(AudioFormat::Aac),
        _ => None,
    }
}

fn parse_subtitle_format(value: &str) -> Option<SubtitleFormat> {
    match value.to_ascii_lowercase().as_str() {
        "srt" => Some(SubtitleFormat::Srt),
        "vtt" | "webvtt" => Some(SubtitleFormat::WebVtt),
        "raw" | "txt" => Some(SubtitleFormat::Raw),
        _ => None,
    }
}

fn parse_timecode(value: &str) -> Result<Duration, Box<dyn std::error::Error>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("time value cannot be empty".into());
    }

    if let Ok(seconds) = trimmed.parse::<f64>() {
        return Ok(Duration::from_secs_f64(seconds.max(0.0)));
    }

    let parts: Vec<&str> = trimmed.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return Err(format!("invalid time format: {trimmed}").into());
    }

    let (hours, minutes, seconds_str) = if parts.len() == 3 {
        (parts[0].parse::<u64>()?, parts[1].parse::<u64>()?, parts[2])
    } else {
        (0_u64, parts[0].parse::<u64>()?, parts[1])
    };

    let seconds = seconds_str.parse::<f64>()?;
    let total_seconds = (hours as f64 * 3600.0) + (minutes as f64 * 60.0) + seconds;
    Ok(Duration::from_secs_f64(total_seconds.max(0.0)))
}

fn timestamp_to_frame_number(timestamp: Duration, frames_per_second: f64) -> u64 {
    (timestamp.as_secs_f64() * frames_per_second) as u64
}

fn open_input(input: &str) -> Result<MediaFile, Box<dyn std::error::Error>> {
    if input.contains("://") {
        Ok(MediaFile::open_url(input)?)
    } else {
        Ok(MediaFile::open(input)?)
    }
}

fn parse_pixel_format(value: &str) -> Option<PixelFormat> {
    match value.to_ascii_lowercase().as_str() {
        "rgb8" | "rgb" => Some(PixelFormat::Rgb8),
        "rgba8" | "rgba" => Some(PixelFormat::Rgba8),
        "gray8" | "gray" | "greyscale" | "grayscale" => Some(PixelFormat::Gray8),
        _ => None,
    }
}

fn parse_log_level(value: &str) -> Option<FfmpegLogLevel> {
    match value.to_ascii_lowercase().as_str() {
        "quiet" => Some(FfmpegLogLevel::Quiet),
        "panic" => Some(FfmpegLogLevel::Panic),
        "fatal" => Some(FfmpegLogLevel::Fatal),
        "error" => Some(FfmpegLogLevel::Error),
        "warning" | "warn" => Some(FfmpegLogLevel::Warning),
        "info" => Some(FfmpegLogLevel::Info),
        "verbose" => Some(FfmpegLogLevel::Verbose),
        "debug" => Some(FfmpegLogLevel::Debug),
        "trace" => Some(FfmpegLogLevel::Trace),
        _ => None,
    }
}

#[cfg(feature = "hardware")]
fn parse_hardware_mode(value: &str) -> Option<HardwareAccelerationMode> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => Some(HardwareAccelerationMode::Auto),
        "software" | "sw" | "cpu" => Some(HardwareAccelerationMode::Software),
        "cuda" => Some(HardwareAccelerationMode::Specific(HardwareDeviceType::Cuda)),
        "vaapi" => Some(HardwareAccelerationMode::Specific(
            HardwareDeviceType::Vaapi,
        )),
        "dxva2" => Some(HardwareAccelerationMode::Specific(
            HardwareDeviceType::Dxva2,
        )),
        "d3d11va" => Some(HardwareAccelerationMode::Specific(
            HardwareDeviceType::D3d11va,
        )),
        "videotoolbox" => Some(HardwareAccelerationMode::Specific(
            HardwareDeviceType::VideoToolbox,
        )),
        "qsv" => Some(HardwareAccelerationMode::Specific(HardwareDeviceType::Qsv)),
        _ => None,
    }
}

fn ensure_writable_path(path: &Path, overwrite: bool) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        if overwrite {
            eprintln!(
                "{} {}",
                "warning:".yellow().bold(),
                format!("overwriting {}", path.display()).yellow()
            );
        } else {
            return Err(format!(
                "output already exists: {} (use --overwrite to replace)",
                path.display()
            )
            .into());
        }
    }
    Ok(())
}

fn base_extract_options(
    global: &GlobalOptions,
) -> Result<ExtractOptions, Box<dyn std::error::Error>> {
    let mut options = ExtractOptions::new();

    if let Some(pixel_str) = &global.pixel_format {
        let pixel = parse_pixel_format(pixel_str)
            .ok_or(format!("unsupported --pixel-format: {pixel_str}"))?;
        options = options.with_pixel_format(pixel);
    }

    #[cfg(feature = "hardware")]
    if let Some(hardware) = &global.hardware {
        let mode = parse_hardware_mode(hardware)
            .ok_or(format!("unsupported --hardware mode: {hardware}"))?;
        options = options.with_hardware_acceleration(mode);
    }

    if global.progress {
        options = options.with_progress(Arc::new(TerminalProgress::new()));
    }

    Ok(options)
}

fn apply_global_options(global: &GlobalOptions) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(level) = &global.log_level {
        let parsed = parse_log_level(level).ok_or(format!("unsupported --log-level: {level}"))?;
        unbundle::set_ffmpeg_log_level(parsed);
    }

    if let Some(threads) = global.threads {
        if threads > 0 {
            unsafe {
                std::env::set_var("RAYON_NUM_THREADS", threads.to_string());
            }
        }
    }

    #[cfg(not(feature = "hardware"))]
    if global.hardware.is_some() {
        eprintln!(
            "{} {}",
            "warning:".yellow().bold(),
            "--hardware requires building with the `hardware` feature".yellow()
        );
    }

    Ok(())
}

#[derive(Default)]
struct TerminalProgress;

impl TerminalProgress {
    fn new() -> Self {
        Self
    }
}

impl ProgressCallback for TerminalProgress {
    fn on_progress(&self, info: &ProgressInfo) {
        if let Some(total) = info.total {
            eprintln!("{} {}/{}", "progress".cyan().bold(), info.current, total);
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    apply_global_options(&cli.global)?;

    match cli.command {
        Commands::Metadata { input, json } => {
            let unbundler = open_input(&input)?;
            let metadata = unbundler.metadata();
            if json {
                let payload = json!({
                    "format": metadata.format,
                    "duration_seconds": metadata.duration.as_secs_f64(),
                    "video": metadata.video.as_ref().map(|video| json!({
                        "width": video.width,
                        "height": video.height,
                        "fps": video.frames_per_second,
                        "frame_count": video.frame_count,
                        "codec": video.codec,
                    })),
                    "audio": metadata.audio.as_ref().map(|audio| json!({
                        "sample_rate": audio.sample_rate,
                        "channels": audio.channels,
                        "codec": audio.codec,
                        "bit_rate": audio.bit_rate,
                    })),
                    "subtitle": metadata.subtitle.as_ref().map(|sub| json!({
                        "codec": sub.codec,
                        "language": sub.language,
                    })),
                    "chapters": metadata.chapters.as_ref().map(|chapters| chapters.len()).unwrap_or(0),
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("Format: {}", metadata.format);
                println!("Duration: {:?}", metadata.duration);
                if let Some(chapters) = &metadata.chapters {
                    println!("Chapters: {}", chapters.len());
                }
                if let Some(video) = &metadata.video {
                    println!(
                        "Video: {}x{} @ {:.2} fps [{}]",
                        video.width, video.height, video.frames_per_second, video.codec,
                    );
                }
                if let Some(audio) = &metadata.audio {
                    println!(
                        "Audio: {} Hz, {} ch [{}]",
                        audio.sample_rate, audio.channels, audio.codec,
                    );
                }
                if let Some(subtitle) = &metadata.subtitle {
                    println!("Subtitle: {}", subtitle.codec);
                }
            }
        }
        Commands::ExtractFrames {
            input,
            out,
            every,
            start,
            end,
            ext,
        } => {
            if every == 0 {
                return Err("--every must be greater than 0".into());
            }

            if out.exists() {
                if !cli.global.overwrite {
                    return Err(format!(
                        "output directory already exists: {} (use --overwrite)",
                        out.display()
                    )
                    .into());
                }
                eprintln!(
                    "{} {}",
                    "warning:".yellow().bold(),
                    format!("writing into existing directory {}", out.display()).yellow()
                );
            }
            fs::create_dir_all(&out)?;

            let mut unbundler = open_input(&input)?;
            let metadata = unbundler
                .metadata()
                .video
                .clone()
                .ok_or("No video stream")?;
            let max_frame = metadata.frame_count.saturating_sub(1);

            let start_frame = if let Some(start) = start {
                if start.contains(':') {
                    let start_time = parse_timecode(&start)?;
                    timestamp_to_frame_number(start_time, metadata.frames_per_second).min(max_frame)
                } else {
                    start.parse::<u64>()?.min(max_frame)
                }
            } else {
                0
            };

            let end_frame = if let Some(end) = end {
                if end.contains(':') {
                    let end_time = parse_timecode(&end)?;
                    timestamp_to_frame_number(end_time, metadata.frames_per_second).min(max_frame)
                } else {
                    end.parse::<u64>()?.min(max_frame)
                }
            } else {
                max_frame
            };

            if start_frame > end_frame {
                return Err("--start must be <= --end".into());
            }

            let frame_numbers: Vec<u64> =
                (start_frame..=end_frame).step_by(every as usize).collect();

            let ext_clean = ext.trim_start_matches('.').to_ascii_lowercase();
            let mut extracted = 0_u64;
            let options = base_extract_options(&cli.global)?;

            let progress_bar = if cli.global.progress {
                let pb = ProgressBar::new(frame_numbers.len() as u64);
                let style = ProgressStyle::with_template(
                    "{spinner:.green} {bar:40.cyan/blue} {pos}/{len} {msg}",
                )?;
                pb.set_style(style.progress_chars("##-"));
                Some(pb)
            } else {
                None
            };

            let mut handle = unbundler.video();
            for frame_number in frame_numbers {
                let output_path = out.join(format!("frame_{frame_number:06}.{ext_clean}"));
                if output_path.exists() && !cli.global.overwrite {
                    return Err(format!(
                        "output file already exists: {} (use --overwrite)",
                        output_path.display()
                    )
                    .into());
                }

                let image = handle.frame_with_options(frame_number, &options)?;
                image.save(&output_path)?;
                extracted += 1;

                if let Some(pb) = &progress_bar {
                    pb.inc(1);
                }

                if cli.global.verbose {
                    eprintln!("saved frame {} -> {}", frame_number, output_path.display());
                }
            }

            if let Some(pb) = progress_bar {
                pb.finish_with_message("done");
            }

            println!(
                "{} {}",
                "success:".green().bold(),
                format!("Extracted {extracted} frame(s) to {}", out.display()).green()
            );
        }
        Commands::ExtractAudio {
            input,
            format,
            out,
            start,
            end,
        } => {
            let audio_format =
                parse_audio_format(&format).ok_or("Unsupported --format for audio")?;

            ensure_writable_path(&out, cli.global.overwrite)?;
            let mut unbundler = open_input(&input)?;

            match (start, end) {
                (Some(start_time), Some(end_time)) => {
                    unbundler.audio().save_range(
                        &out,
                        parse_timecode(&start_time)?,
                        parse_timecode(&end_time)?,
                        audio_format,
                    )?;
                }
                (None, None) => {
                    unbundler.audio().save(&out, audio_format)?;
                }
                _ => {
                    return Err("Provide both --start and --end, or neither".into());
                }
            }

            println!("{} {}", "saved".green().bold(), out.display());
        }
        Commands::ExtractSubs {
            input,
            format,
            out,
            start,
            end,
        } => {
            let subtitle_format =
                parse_subtitle_format(&format).ok_or("Unsupported --format for subtitles")?;

            ensure_writable_path(&out, cli.global.overwrite)?;
            let mut unbundler = open_input(&input)?;

            match (start, end) {
                (Some(start_time), Some(end_time)) => {
                    unbundler.subtitle().save_range(
                        &out,
                        subtitle_format,
                        parse_timecode(&start_time)?,
                        parse_timecode(&end_time)?,
                    )?;
                }
                (None, None) => {
                    unbundler.subtitle().save(&out, subtitle_format)?;
                }
                _ => {
                    return Err("Provide both --start and --end, or neither".into());
                }
            }

            println!("{} {}", "saved".green().bold(), out.display());
        }
        Commands::Thumbnail {
            input,
            out,
            mode,
            timestamp,
            frame,
            columns,
            rows,
            samples,
            max_dimension,
        } => {
            ensure_writable_path(&out, cli.global.overwrite)?;

            let mut unbundler = open_input(&input)?;
            let image = match mode.to_ascii_lowercase().as_str() {
                "single" => {
                    if let Some(frame_number) = frame {
                        unbundle::ThumbnailHandle::at_frame(
                            &mut unbundler,
                            frame_number,
                            max_dimension,
                        )?
                    } else {
                        let timestamp = timestamp.unwrap_or_else(|| "0".to_string());
                        unbundle::ThumbnailHandle::at_timestamp(
                            &mut unbundler,
                            parse_timecode(&timestamp)?,
                            max_dimension,
                        )?
                    }
                }
                "grid" => {
                    let options = unbundle::ThumbnailOptions::new(columns, rows);
                    unbundle::ThumbnailHandle::grid(&mut unbundler, &options)?
                }
                "smart" => {
                    unbundle::ThumbnailHandle::smart(&mut unbundler, samples, max_dimension)?
                }
                _ => return Err("unsupported --mode (single|grid|smart)".into()),
            };

            image.save(&out)?;
            println!("{} {}", "saved".green().bold(), out.display());
        }
        Commands::Remux {
            input,
            output,
            exclude_video,
            exclude_audio,
            exclude_subtitles,
        } => {
            ensure_writable_path(&output, cli.global.overwrite)?;
            let mut remuxer = unbundle::Remuxer::new(input, &output)?;
            if exclude_video {
                remuxer = remuxer.exclude_video();
            }
            if exclude_audio {
                remuxer = remuxer.exclude_audio();
            }
            if exclude_subtitles {
                remuxer = remuxer.exclude_subtitles();
            }
            remuxer.run()?;
            println!("{} {}", "saved".green().bold(), output.display());
        }
        Commands::Validate { input } => {
            let unbundler = open_input(&input)?;
            let report = unbundler.validate();
            print!("{report}");
        }
        #[cfg(feature = "scene")]
        Commands::SceneDetect {
            input,
            threshold,
            json,
        } => {
            let mut unbundler = open_input(&input)?;
            let changes = unbundler
                .video()
                .detect_scenes(Some(SceneDetectionOptions::new().threshold(threshold)))?;

            if json {
                let payload: Vec<_> = changes
                    .iter()
                    .map(|change| {
                        json!({
                            "timestamp_seconds": change.timestamp.as_secs_f64(),
                            "frame_number": change.frame_number,
                            "score": change.score,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                for change in changes {
                    println!(
                        "cut at {:.3}s (frame {}, score {:.2})",
                        change.timestamp.as_secs_f64(),
                        change.frame_number,
                        change.score
                    );
                }
            }
        }
        #[cfg(feature = "waveform")]
        Commands::Waveform { input, bins, out } => {
            let mut unbundler = open_input(&input)?;
            let waveform = unbundler
                .audio()
                .generate_waveform(&WaveformOptions::new().bins(bins))?;
            if let Some(path) = out {
                ensure_writable_path(&path, cli.global.overwrite)?;
                let mut file = fs::File::create(&path)?;
                writeln!(file, "index,min,max,rms")?;
                for (index, bin) in waveform.bins.iter().enumerate() {
                    writeln!(file, "{index},{},{},{}", bin.min, bin.max, bin.rms)?;
                }
                println!("{} {}", "saved".green().bold(), path.display());
            } else {
                println!(
                    "bins={} duration={:.3}s samples={}",
                    waveform.bins.len(),
                    waveform.duration.as_secs_f64(),
                    waveform.total_samples
                );
            }
        }
        #[cfg(feature = "loudness")]
        Commands::Loudness { input, json } => {
            let mut unbundler = open_input(&input)?;
            let info: LoudnessInfo = unbundler.audio().analyze_loudness()?;
            if json {
                let payload = json!({
                    "peak": info.peak,
                    "peak_dbfs": info.peak_dbfs,
                    "rms": info.rms,
                    "rms_dbfs": info.rms_dbfs,
                    "duration_seconds": info.duration.as_secs_f64(),
                    "total_samples": info.total_samples,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("Peak: {:.2} dBFS", info.peak_dbfs);
                println!("RMS: {:.2} dBFS", info.rms_dbfs);
            }
        }
        Commands::Completions { shell } => {
            let mut command = Cli::command();
            clap_complete::generate(shell, &mut command, "unbundle", &mut std::io::stdout());
        }
    }

    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_audio_format, parse_subtitle_format, parse_timecode};

    #[test]
    fn parse_audio_format_aliases() {
        assert!(parse_audio_format("wav").is_some());
        assert!(parse_audio_format("mp3").is_some());
        assert!(parse_audio_format("FLAC").is_some());
        assert!(parse_audio_format("aac").is_some());
        assert!(parse_audio_format("ogg").is_none());
    }

    #[test]
    fn parse_subtitle_format_aliases() {
        assert!(parse_subtitle_format("srt").is_some());
        assert!(parse_subtitle_format("vtt").is_some());
        assert!(parse_subtitle_format("webvtt").is_some());
        assert!(parse_subtitle_format("raw").is_some());
        assert!(parse_subtitle_format("txt").is_some());
        assert!(parse_subtitle_format("ass").is_none());
    }

    #[test]
    fn parse_timecode_formats() {
        let seconds = parse_timecode("75").unwrap();
        assert_eq!(seconds.as_secs(), 75);

        let mm_ss = parse_timecode("01:15").unwrap();
        assert_eq!(mm_ss.as_secs(), 75);

        let hh_mm_ss = parse_timecode("00:01:15.5").unwrap();
        assert_eq!(hh_mm_ss.as_secs(), 75);
    }
}
