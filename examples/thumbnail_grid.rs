//! Create a thumbnail grid from evenly-spaced video frames.
//!
//! Extracts N frames at regular intervals and composites them into a single
//! grid image.
//!
//! Usage:
//!   cargo run --example thumbnail_grid -- <input_file> [columns] [rows]

use std::error::Error;

use image::{DynamicImage, GenericImage};
use unbundle::{FrameRange, MediaUnbundler};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());
    let columns: u32 = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);
    let rows: u32 = std::env::args()
        .nth(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);

    let total_thumbnails = columns * rows;

    println!("Opening {input_path}...");
    let mut unbundler = MediaUnbundler::open(&input_path)?;

    let metadata = unbundler.metadata();
    let video_metadata = metadata
        .video
        .as_ref()
        .expect("Input file has no video stream");

    let frame_count = video_metadata.frame_count;
    let thumbnail_width = video_metadata.width;
    let thumbnail_height = video_metadata.height;

    println!(
        "Video: {}x{}, {} frames, creating {}x{} grid",
        thumbnail_width, thumbnail_height, frame_count, columns, rows,
    );

    // Calculate evenly-spaced frame numbers.
    let step = if frame_count > total_thumbnails as u64 {
        frame_count / total_thumbnails as u64
    } else {
        1
    };
    let frame_numbers: Vec<u64> = (0..total_thumbnails as u64)
        .map(|index| index * step)
        .filter(|number| *number < frame_count)
        .collect();

    println!("Extracting {} frames...", frame_numbers.len());
    let frames = unbundler
        .video()
        .frames(FrameRange::Specific(frame_numbers))?;

    // Scale thumbnails to a reasonable size for the grid.
    let scale_factor = 320.0 / thumbnail_width as f64;
    let scaled_width = (thumbnail_width as f64 * scale_factor) as u32;
    let scaled_height = (thumbnail_height as f64 * scale_factor) as u32;

    // Create the grid image.
    let grid_width = scaled_width * columns;
    let grid_height = scaled_height * rows;
    let mut grid = DynamicImage::new_rgb8(grid_width, grid_height);

    for (index, frame) in frames.iter().enumerate() {
        let column = (index as u32) % columns;
        let row = (index as u32) / columns;
        if row >= rows {
            break;
        }

        let thumbnail = frame.resize_exact(
            scaled_width,
            scaled_height,
            image::imageops::FilterType::Triangle,
        );

        let horizontal_position = column * scaled_width;
        let vertical_position = row * scaled_height;
        grid.copy_from(&thumbnail, horizontal_position, vertical_position)?;
    }

    let output_path = "thumbnail_grid.png";
    grid.save(output_path)?;
    println!("Saved {output_path} ({grid_width}x{grid_height})");

    Ok(())
}
