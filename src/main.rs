#![feature(portable_simd)]

mod auth;
mod cli;
mod config;
mod filter;
mod guard;
mod http;
mod keyboard;
mod mouse;
mod recording;
mod screenshot;
mod server;
mod tls;

use anyhow::{Context, Result};
use clap::Parser;
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

    let cli = cli::Cli::parse();
    if let Some(cli::Command::Auth { cmd }) = &cli.command {
        return cli::run_auth(cmd);
    }

    check_resolution().await?;

    tracing::info!(
        "Starting computer-use MCP server ({}x{} → {}x{})",
        DISPLAY_WIDTH,
        DISPLAY_HEIGHT,
        SCALED_WIDTH,
        SCALED_HEIGHT
    );

    let config = Config::load(&cli)?;
    tracing::info!(
        transport = ?config.transport,
        host = %config.host,
        port = config.port,
        auth_enabled = config.auth_key.is_some(),
        ip_allowlist = %config.ip_allowlist.join(","),
        cors_origins = %config.cors_origins.join(","),
        tool_allow = %config.tool_allow.join(","),
        tool_deny = %config.tool_deny.join(","),
        tls_cert = ?config.tls_cert,
        tls_key = ?config.tls_key,
        allow_insecure_remote = config.allow_insecure_remote,
        config_path = ?config.config_path,
        "configuration"
    );

    if config.transport == Transport::Http {
        guard::validate_bind(
            config.auth_key.as_deref().unwrap_or_default(),
            &config.host,
            config.tls_cert.as_deref().and_then(|p| p.to_str()),
            config.allow_insecure_remote,
        )
        .map_err(anyhow::Error::msg)?;
    }

    match config.transport {
        Transport::Stdio => {
            let server = ComputerUseServer::filtered(&config)?;
            let service = server.serve(stdio()).await?;
            service.waiting().await?;
        }
        Transport::Http => {
            http::serve(config).await?;
        }
    }

    Ok(())
}
