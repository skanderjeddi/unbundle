//! Scene change detection example (feature = "scene-detection").
//!
//! Usage:
//!   cargo run --features=scene-detection --example scene_detection -- <input_file>

use std::error::Error;

use unbundle::{MediaUnbundler, SceneDetectionConfig};

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    println!("Opening {input_path}...");
    let mut unbundler = MediaUnbundler::open(&input_path)?;

    // Use the default threshold (10.0) for scene detection.
    let config = SceneDetectionConfig::default();
    println!(
        "Detecting scenes (threshold {:.1})...",
        config.threshold,
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
