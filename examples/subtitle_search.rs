//! Search subtitles and display extended container metadata.
//!
//! Usage: `cargo run --example subtitle_search -- path/to/video_with_subs.mkv`

use std::time::Duration;

use unbundle::MediaUnbundler;

fn main() -> Result<(), unbundle::UnbundleError> {
    let path = std::env::args().nth(1).expect("Usage: subtitle_search <video_path>");

    let mut unbundler = MediaUnbundler::open(&path)?;

    // Display container tags.
    let meta = unbundler.metadata();
    if let Some(tags) = &meta.tags {
        println!("Container tags:");
        for (key, value) in tags {
            println!("  {key}: {value}");
        }
    } else {
        println!("No container tags found.");
    }

    // Display colorspace info.
    if let Some(video) = &meta.video {
        println!("\nColorspace info:");
        println!("  Color space: {:?}", video.color_space);
        println!("  Color range: {:?}", video.color_range);
        println!("  Color primaries: {:?}", video.color_primaries);
        println!("  Color transfer: {:?}", video.color_transfer);
        println!("  Bits per raw sample: {:?}", video.bits_per_raw_sample);
        println!("  Pixel format: {:?}", video.pixel_format_name);
    }

    // Search subtitles.
    if meta.subtitle.is_some() {
        println!("\nSearching subtitles for 'hello'...");
        let results = unbundler.subtitle().search("hello")?;
        if results.is_empty() {
            println!("  No matches found.");
        } else {
            for event in &results {
                println!(
                    "  [{:?} – {:?}] {}",
                    event.start_time, event.end_time, event.text
                );
            }
        }

        // Time-range extraction.
        println!("\nSubtitles in first 3 seconds:");
        let range = unbundler
            .subtitle()
            .extract_range(Duration::ZERO, Duration::from_secs(3))?;
        for event in &range {
            println!(
                "  [{:?} – {:?}] {}",
                event.start_time, event.end_time, event.text
            );
        }
    } else {
        println!("\nNo subtitle stream found.");
    }

    Ok(())
}
