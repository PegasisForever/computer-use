use crate::config::*;
use anyhow::{Context, Result};
use tokio::process::Command;
use tokio::time::{Duration, sleep};

/// Scale coordinates from AI model space (1456x819) to display space (1920x1080).
fn scale_coords(x: f64, y: f64) -> (i32, i32) {
    let screen_x = (x * DISPLAY_WIDTH as f64 / SCALED_WIDTH as f64).round() as i32;
    let screen_y = (y * DISPLAY_HEIGHT as f64 / SCALED_HEIGHT as f64).round() as i32;
    (screen_x, screen_y)
}

/// Quadratic ease-in-out interpolation.
fn ease_in_out(t: f64) -> f64 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        -1.0 + (4.0 - 2.0 * t) * t
    }
}

/// Get current mouse position in screen coordinates.
pub async fn get_mouse_position() -> Result<(i32, i32)> {
    let output = Command::new("xdotool")
        .args(["getmouselocation", "--shell"])
        .output()
        .await
        .context("failed to run xdotool getmouselocation")?;

    let stdout = String::from_utf8(output.stdout)?;
    let mut x = 0i32;
    let mut y = 0i32;

    for line in stdout.lines() {
        if let Some(val) = line.strip_prefix("X=") {
            x = val.parse().context("failed to parse X coordinate")?;
        } else if let Some(val) = line.strip_prefix("Y=") {
            y = val.parse().context("failed to parse Y coordinate")?;
        }
    }

    Ok((x, y))
}

/// Smoothly move the mouse from current position to target using ease-in-out.
pub async fn smooth_move(to_x: i32, to_y: i32) -> Result<()> {
    let (from_x, from_y) = get_mouse_position().await?;
    let step_duration = Duration::from_millis(MOUSE_MOVE_DURATION_MS / MOUSE_MOVE_STEPS as u64);

    for i in 1..=MOUSE_MOVE_STEPS {
        let t = i as f64 / MOUSE_MOVE_STEPS as f64;
        let eased = ease_in_out(t);
        let x = from_x as f64 + (to_x - from_x) as f64 * eased;
        let y = from_y as f64 + (to_y - from_y) as f64 * eased;

        Command::new("xdotool")
            .args([
                "mousemove",
                &(x.round() as i32).to_string(),
                &(y.round() as i32).to_string(),
            ])
            .status()
            .await
            .context("failed to run xdotool mousemove")?;

        sleep(step_duration).await;
    }

    Ok(())
}

/// Move mouse to scaled coordinates if provided.
pub async fn maybe_move(x: Option<f64>, y: Option<f64>) -> Result<()> {
    if let (Some(x), Some(y)) = (x, y) {
        let (sx, sy) = scale_coords(x, y);
        smooth_move(sx, sy).await?;
    }
    Ok(())
}

/// Move mouse to scaled coordinates.
pub async fn move_to_scaled(x: f64, y: f64) -> Result<()> {
    let (sx, sy) = scale_coords(x, y);
    smooth_move(sx, sy).await
}

/// Press left mouse button down.
pub async fn left_mouse_down() -> Result<()> {
    Command::new("xdotool")
        .args(["mousedown", "1"])
        .status()
        .await
        .context("failed to run xdotool mousedown")?;
    Ok(())
}

/// Release left mouse button.
pub async fn left_mouse_up() -> Result<()> {
    Command::new("xdotool")
        .args(["mouseup", "1"])
        .status()
        .await
        .context("failed to run xdotool mouseup")?;
    Ok(())
}

/// Double-click the left mouse button.
pub async fn left_double_click() -> Result<()> {
    Command::new("xdotool")
        .args(["click", "--repeat", "2", "1"])
        .status()
        .await
        .context("failed to run xdotool double-click")?;
    Ok(())
}

/// Click the right mouse button.
pub async fn right_click() -> Result<()> {
    Command::new("xdotool")
        .args(["click", "3"])
        .status()
        .await
        .context("failed to run xdotool right-click")?;
    Ok(())
}

/// Click the middle mouse button.
pub async fn middle_click() -> Result<()> {
    Command::new("xdotool")
        .args(["click", "2"])
        .status()
        .await
        .context("failed to run xdotool middle-click")?;
    Ok(())
}

/// Perform smooth scrolling. Positive amount scrolls down, negative up.
pub async fn scroll(amount: i32) -> Result<()> {
    let button = if amount > 0 { "5" } else { "4" };
    let steps = amount.unsigned_abs();
    let step_delay = Duration::from_millis(SCROLL_STEP_DELAY_MS);

    for _ in 0..steps {
        Command::new("xdotool")
            .args(["click", button])
            .status()
            .await
            .context("failed to run xdotool scroll")?;
        sleep(step_delay).await;
    }

    Ok(())
}
