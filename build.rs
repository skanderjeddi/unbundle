use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    println!("cargo:rerun-if-env-changed=VCPKGRS_DYNAMIC");
    println!("cargo:rerun-if-env-changed=VCPKGRS_TRIPLET");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    if env::var_os("FFMPEG_DIR").is_some() {
        return;
    }

    let vcpkg_root = match env::var("VCPKG_ROOT") {
        Ok(value) => value,
        Err(_) => {
            println!(
                "cargo:warning=FFMPEG_DIR is not set. On Windows, install FFmpeg via vcpkg and set VCPKG_ROOT + FFMPEG_DIR for reliable builds."
            );
            return;
        }
    };

    let triplet = env::var("VCPKGRS_TRIPLET").unwrap_or_else(|_| "x64-windows".to_string());
    let ffmpeg_dir = PathBuf::from(&vcpkg_root).join("installed").join(&triplet);

    if ffmpeg_dir.exists() {
        println!(
            "cargo:warning=Detected vcpkg FFmpeg at {}. Set FFMPEG_DIR={} to make ffmpeg-sys-next discovery explicit.",
            ffmpeg_dir.display(),
            ffmpeg_dir.display(),
        );
        if env::var_os("VCPKGRS_DYNAMIC").is_none() {
            println!(
                "cargo:warning=Consider setting VCPKGRS_DYNAMIC=1 when using vcpkg dynamic FFmpeg builds on Windows."
            );
        }
    } else {
        println!(
            "cargo:warning=VCPKG_ROOT is set but no FFmpeg install was found at {}.",
            ffmpeg_dir.display(),
        );
    }
}
