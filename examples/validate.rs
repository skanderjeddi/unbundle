//! Validate a media file and display the report.
//!
//! Usage:
//!   cargo run --example validate -- <input_file>

use std::error::Error;

use unbundle::MediaFile;

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input.mp4".to_string());

    let unbundler = MediaFile::open(&input_path)?;

    let report = unbundler.validate();

    println!("Validation report for {input_path}:");
    println!("{report}");

    println!("Valid: {}", report.is_valid());
    println!("Total issues: {}", report.issue_count());

    if !report.info.is_empty() {
        println!("\nInfo ({}):", report.info.len());
        for item in &report.info {
            println!("  {item}");
        }
    }

    if !report.warnings.is_empty() {
        println!("\nWarnings ({}):", report.warnings.len());
        for item in &report.warnings {
            println!("  {item}");
        }
    }

    if !report.errors.is_empty() {
        println!("\nErrors ({}):", report.errors.len());
        for item in &report.errors {
            println!("  {item}");
        }
    }

    Ok(())
}
