use anyhow::{Context, Result};
use tokio::process::Command;

/// Press a key or key combination (e.g. "ctrl+c", "Return", "alt+Tab").
pub async fn key_press(keys: &str) -> Result<()> {
    let status = Command::new("xdotool")
        .args(["key", "--clearmodifiers", keys])
        .status()
        .await
        .context("failed to run xdotool key")?;

    if !status.success() {
        anyhow::bail!("xdotool key exited with status: {}", status);
    }
    Ok(())
}

/// Type a string of text on the keyboard.
pub async fn type_text(text: &str) -> Result<()> {
    let status = Command::new("xdotool")
        .args(["type", "--clearmodifiers", "--delay", "12", text])
        .status()
        .await
        .context("failed to run xdotool type")?;

    if !status.success() {
        anyhow::bail!("xdotool type exited with status: {}", status);
    }
    Ok(())
}
