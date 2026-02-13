//! Open media from a URL or path-like source string.
//!
//! Usage:
//!   cargo run --example open_url -- <source>
//!
//! Examples:
//!   cargo run --example open_url -- tests/fixtures/sample_video.mp4
//!   cargo run --example open_url -- https://example.com/video.mp4

use std::error::Error;

use unbundle::MediaFile;

fn main() -> Result<(), Box<dyn Error>> {
    let source = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tests/fixtures/sample_video.mp4".to_string());

    println!("Opening source via open_url: {source}");
    let mut unbundler = MediaFile::open_url(&source)?;

    let metadata = unbundler.metadata();
    println!("Format:   {}", metadata.format);
    println!("Duration: {:?}", metadata.duration);

    if let Some(video) = &metadata.video {
        println!(
            "Video:    {}x{} @ {:.2} fps ({})",
            video.width, video.height, video.frames_per_second, video.codec,
        );

        let frame = unbundler.video().frame(0)?;
        frame.save("open_url_first_frame.png")?;
        println!("Saved open_url_first_frame.png");
    } else {
        println!("No video stream found in source");
    }

    Ok(())
}
