use std::{fs, path::PathBuf, time::Duration};

use clap::{Parser, Subcommand};
use unbundle::{AudioFormat, FrameRange, MediaFile, SubtitleFormat};

#[derive(Debug, Parser)]
#[command(
    name = "unbundle-cli",
    version,
    about = "Extract frames, audio, subtitles, and metadata from media files"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Probe a media file and print a metadata summary.
    Probe {
        /// Input media path or URL.
        input: String,
    },
    /// Extract frames to an output directory.
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
        start: Option<u64>,
        /// Optional end frame (inclusive).
        #[arg(long)]
        end: Option<u64>,
        /// Output image extension (png, jpg, jpeg, bmp, tiff).
        #[arg(long, default_value = "png")]
        ext: String,
    },
    /// Extract audio track to a file.
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
        start: Option<f64>,
        /// Optional end time in seconds.
        #[arg(long)]
        end: Option<f64>,
    },
    /// Extract subtitles to a file.
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
        start: Option<f64>,
        /// Optional end time in seconds.
        #[arg(long)]
        end: Option<f64>,
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

fn parse_seconds(value: f64) -> Duration {
    Duration::from_secs_f64(value.max(0.0))
}

fn open_input(input: &str) -> Result<MediaFile, Box<dyn std::error::Error>> {
    if input.contains("://") {
        Ok(MediaFile::open_url(input)?)
    } else {
        Ok(MediaFile::open(input)?)
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Probe { input } => {
            let unbundler = open_input(&input)?;
            let metadata = unbundler.metadata();
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

            fs::create_dir_all(&out)?;

            let mut unbundler = open_input(&input)?;
            let metadata = unbundler
                .metadata()
                .video
                .clone()
                .ok_or("No video stream")?;
            let max_frame = metadata.frame_count.saturating_sub(1);
            let start_frame = start.unwrap_or(0).min(max_frame);
            let end_frame = end.unwrap_or(max_frame).min(max_frame);

            if start_frame > end_frame {
                return Err("--start must be <= --end".into());
            }

            let frame_numbers: Vec<u64> =
                (start_frame..=end_frame).step_by(every as usize).collect();

            let ext_clean = ext.trim_start_matches('.').to_ascii_lowercase();
            let mut extracted = 0_u64;

            unbundler.video().for_each_frame(
                FrameRange::Specific(frame_numbers),
                |frame_number, image| {
                    let output_path = out.join(format!("frame_{frame_number:06}.{ext_clean}"));
                    image.save(&output_path)?;
                    extracted += 1;
                    Ok(())
                },
            )?;

            println!("Extracted {extracted} frame(s) to {}", out.display());
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
            let mut unbundler = open_input(&input)?;

            match (start, end) {
                (Some(start_seconds), Some(end_seconds)) => {
                    unbundler.audio().save_range(
                        &out,
                        parse_seconds(start_seconds),
                        parse_seconds(end_seconds),
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

            println!("Saved {}", out.display());
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
            let mut unbundler = open_input(&input)?;

            match (start, end) {
                (Some(start_seconds), Some(end_seconds)) => {
                    unbundler.subtitle().save_range(
                        &out,
                        subtitle_format,
                        parse_seconds(start_seconds),
                        parse_seconds(end_seconds),
                    )?;
                }
                (None, None) => {
                    unbundler.subtitle().save(&out, subtitle_format)?;
                }
                _ => {
                    return Err("Provide both --start and --end, or neither".into());
                }
            }

            println!("Saved {}", out.display());
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
    use super::{parse_audio_format, parse_subtitle_format};

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
}
