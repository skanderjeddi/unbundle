//! Scene change detection example (feature = "scene").
//!
//! Usage:
//!   cargo run --features=scene --example scene -- <input_file>

use std::error::Error;
use std::time::Duration;

use unbundle::{MediaFile, SceneDetectionMode, SceneDetectionOptions};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    println!("Opening {input_path}...");
    let mut unbundler = MediaFile::open(&input_path)?;

    // Bound analysis for predictable latency on long videos.
    let config = SceneDetectionOptions::new()
        .mode(SceneDetectionMode::Full)
        .threshold(10.0)
        .max_duration(Duration::from_secs(120))
        .max_scene_changes(100);
    println!(
        "Detecting scenes (threshold {:.1}, max_duration={:?}, max_scene_changes={:?})...",
        config.threshold,
        config.max_duration,
        config.max_scene_changes,
    );

    let scenes = unbundler.video().detect_scenes(Some(config))?;

    println!("Found {} scene change(s):", scenes.len());
    for (i, scene) in scenes.iter().enumerate() {
        println!(
            "  {:>3}. Frame {:>5}  |  {:.3}s  |  score {:.1}",
            i + 1,
            scene.frame_number,
            scene.timestamp.as_secs_f64(),
            scene.score,
        );
    }

    println!("Done!");
    Ok(())
}
