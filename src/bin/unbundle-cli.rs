use std::time::Duration;

use unbundle::{AudioFormat, MediaFile, SubtitleFormat};

fn print_usage() {
    println!("unbundle-cli (MVP)");
    println!();
    println!("Usage:");
    println!("  unbundle-cli metadata <input>");
    println!("  unbundle-cli frame <input> <frame_number> <output_image>");
    println!("  unbundle-cli frame-at <input> <seconds> <output_image>");
    println!("  unbundle-cli audio <input> <wav|mp3|flac|aac> <output_audio>");
    println!("  unbundle-cli subtitle <input> <srt|vtt|raw> <output_subtitle>");
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

fn open_input(input: &str) -> Result<MediaFile, Box<dyn std::error::Error>> {
    if input.contains("://") {
        Ok(MediaFile::open_url(input)?)
    } else {
        Ok(MediaFile::open(input)?)
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "metadata" => {
            if args.len() != 3 {
                print_usage();
                return Ok(());
            }
            let unbundler = open_input(&args[2])?;
            let metadata = unbundler.metadata();
            println!("Format: {}", metadata.format);
            println!("Duration: {:?}", metadata.duration);
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
        "frame" => {
            if args.len() != 5 {
                print_usage();
                return Ok(());
            }
            let frame_number: u64 = args[3].parse()?;
            let output = &args[4];
            let mut unbundler = open_input(&args[2])?;
            let image = unbundler.video().frame(frame_number)?;
            image.save(output)?;
            println!("Saved {}", output);
        }
        "audio" => {
            if args.len() != 5 {
                print_usage();
                return Ok(());
            }
            let format = parse_audio_format(&args[3]).ok_or("Unsupported audio format")?;
            let output = &args[4];
            let mut unbundler = open_input(&args[2])?;
            unbundler.audio().save(output, format)?;
            println!("Saved {}", output);
        }
        "subtitle" => {
            if args.len() != 5 {
                print_usage();
                return Ok(());
            }
            let format = parse_subtitle_format(&args[3]).ok_or("Unsupported subtitle format")?;
            let output = &args[4];
            let mut unbundler = open_input(&args[2])?;
            unbundler.subtitle().save(output, format)?;
            println!("Saved {}", output);
        }
        "help" | "--help" | "-h" => {
            print_usage();
        }
        "frame-at" => {
            if args.len() != 5 {
                print_usage();
                return Ok(());
            }
            let seconds: f64 = args[3].parse()?;
            let output = &args[4];
            let mut unbundler = open_input(&args[2])?;
            let image = unbundler
                .video()
                .frame_at(Duration::from_secs_f64(seconds.max(0.0)))?;
            image.save(output)?;
            println!("Saved {}", output);
        }
        _ => {
            print_usage();
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
