#![feature(portable_simd)]

mod config;
mod keyboard;
mod mouse;
mod recording;
mod screenshot;
mod server;

use anyhow::{Context, Result};
use config::*;
use rmcp::{ServiceExt, transport::stdio};
use server::ComputerUseServer;
use tokio::process::Command;

/// Verify the X11 display resolution matches the expected 1920x1080.
async fn check_resolution() -> Result<()> {
    let output = Command::new("xrandr")
        .arg("--current")
        .output()
        .await
        .context("failed to run xrandr — is X11 available?")?;

    let stdout = String::from_utf8(output.stdout).context("xrandr output not UTF-8")?;

    let expected = format!("{}x{}", DISPLAY_WIDTH, DISPLAY_HEIGHT);
    let has_correct_resolution = stdout
        .lines()
        .any(|line| line.contains('*') && line.contains(&expected));

    if !has_correct_resolution {
        anyhow::bail!(
            "Display resolution must be {}x{}. Current modes:\n{}",
            DISPLAY_WIDTH,
            DISPLAY_HEIGHT,
            stdout
                .lines()
                .filter(|l| l.contains('*'))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    check_resolution().await?;

    tracing::info!(
        "Starting computer-use MCP server ({}x{} → {}x{})",
        DISPLAY_WIDTH,
        DISPLAY_HEIGHT,
        SCALED_WIDTH,
        SCALED_HEIGHT
    );

    let server = ComputerUseServer::new();
    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}
