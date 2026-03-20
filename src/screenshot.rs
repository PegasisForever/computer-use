use crate::config::*;
use anyhow::{Context, Result};
use base64::Engine;
use image::ImageFormat;
use std::io::Cursor;
use tokio::process::Command;

/// Capture the screen at native resolution using ffmpeg, resize to scaled
/// resolution, and return as a base64-encoded PNG string.
///
/// Uses ffmpeg's x11grab to capture a single frame as PNG to stdout,
/// avoiding any temp files or extra CLI dependencies beyond ffmpeg.
pub async fn capture_screenshot() -> Result<String> {
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());

    let output = Command::new("ffmpeg")
        .args([
            "-f",
            "x11grab",
            "-video_size",
            &format!("{}x{}", DISPLAY_WIDTH, DISPLAY_HEIGHT),
            "-i",
            &display,
            "-frames:v",
            "1",
            "-f",
            "image2",
            "-c:v",
            "png",
            "pipe:1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("failed to run ffmpeg for screenshot")?;

    if !output.status.success() {
        anyhow::bail!("ffmpeg screenshot exited with status: {}", output.status);
    }

    let img = image::load_from_memory(&output.stdout)
        .context("failed to decode screenshot from ffmpeg")?;

    let resized = img.resize_exact(
        SCALED_WIDTH,
        SCALED_HEIGHT,
        image::imageops::FilterType::Lanczos3,
    );

    let mut buf = Cursor::new(Vec::new());
    resized
        .write_to(&mut buf, ImageFormat::Png)
        .context("failed to encode PNG")?;

    Ok(base64::engine::general_purpose::STANDARD.encode(buf.into_inner()))
}
