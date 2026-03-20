use crate::config::*;
use anyhow::{Context, Result};
use base64::Engine;
use image::ImageFormat;
use std::io::Cursor;
use tokio::process::Command;

/// Capture the screen at native resolution, resize to scaled resolution,
/// and return as a base64-encoded PNG string.
pub async fn capture_screenshot() -> Result<String> {
    let tmp_path = format!("/tmp/screenshot_{}.png", rand::random::<u64>());

    let status = Command::new("scrot")
        .arg("-o")
        .arg(&tmp_path)
        .status()
        .await
        .context("failed to run scrot")?;

    if !status.success() {
        anyhow::bail!("scrot exited with status: {}", status);
    }

    let img = image::open(&tmp_path).context("failed to open screenshot")?;
    let _ = tokio::fs::remove_file(&tmp_path).await;

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
