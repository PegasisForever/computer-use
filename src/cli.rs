//! CLI surface: clap parsing plus the `auth gen-key` command.

use crate::config::{CliValues, Transport, default_config_path, write_config_file};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// MCP server providing computer use capabilities on Linux/X11.
#[derive(Debug, Parser)]
#[command(name = "computer-use", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Transport mode: stdio (default) or Streamable HTTP.
    #[arg(long, value_enum, default_value_t = Transport::Stdio)]
    pub transport: Transport,

    /// Address to bind (HTTP mode).
    #[arg(long)]
    pub host: Option<String>,

    /// Port to listen on (HTTP mode).
    #[arg(long)]
    pub port: Option<u16>,

    /// Bearer token required from clients (HTTP mode).
    #[arg(long)]
    pub auth_key: Option<String>,

    /// Comma-separated CIDRs allowed to connect (HTTP mode). Empty allows all.
    #[arg(long)]
    pub ip_allowlist: Option<String>,

    /// Comma-separated allowed origins for CORS (HTTP mode).
    #[arg(long)]
    pub cors_origins: Option<String>,

    /// Comma-separated tool allow list.
    #[arg(long)]
    pub allow_tools: Option<String>,

    /// Comma-separated tool deny list.
    #[arg(long)]
    pub block_tools: Option<String>,

    /// TLS certificate file (requires the `tls` feature).
    #[arg(long)]
    pub tls_cert: Option<PathBuf>,

    /// TLS private key file (requires the `tls` feature).
    #[arg(long)]
    pub tls_key: Option<PathBuf>,

    /// Allow non-loopback binds without TLS (HTTP mode).
    #[arg(long)]
    pub allow_insecure_remote: bool,

    /// Path to a config.toml file (HTTP mode only).
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Auth commands for HTTP transport.
    Auth {
        #[command(subcommand)]
        cmd: AuthArgs,
    },
}

#[derive(Debug, Subcommand)]
pub enum AuthArgs {
    /// Generate a random auth key and print client configuration snippets.
    GenKey {
        /// Also write ~/.config/computer-use/config.toml with the generated key.
        #[arg(long)]
        write_config: bool,
    },
}

impl From<&Cli> for CliValues {
    fn from(cli: &Cli) -> Self {
        Self {
            transport: Some(cli.transport),
            host: cli.host.clone(),
            port: cli.port,
            auth_key: cli.auth_key.clone(),
            ip_allowlist: cli.ip_allowlist.clone(),
            cors_origins: cli.cors_origins.clone(),
            tool_allow: cli.allow_tools.clone(),
            tool_deny: cli.block_tools.clone(),
            tls_cert: cli.tls_cert.as_ref().map(|p| p.display().to_string()),
            tls_key: cli.tls_key.as_ref().map(|p| p.display().to_string()),
            allow_insecure_remote: cli.allow_insecure_remote,
            config_path: cli.config.as_ref().map(|p| p.display().to_string()),
        }
    }
}

/// Run an `auth` subcommand.
pub fn run_auth(auth: &AuthArgs) -> Result<()> {
    match auth {
        AuthArgs::GenKey { write_config } => run_auth_gen_key(*write_config),
    }
}

/// `auth gen-key`: generate a key, print client snippets, optionally write
/// the default config.toml.
fn run_auth_gen_key(write_config: bool) -> Result<()> {
    let token = generate_token();
    print!("{}", format_auth_output(&token));
    if write_config {
        let path = default_config_path();
        write_config_file(&path, &token)
            .with_context(|| format!("failed to write {}", path.display()))?;
        eprintln!("\nWrote config file: {}", path.display());
    }
    Ok(())
}

/// Generate a 32-byte random token encoded as 64 hex characters.
fn generate_token() -> String {
    use rand::TryRng;
    let mut bytes = [0u8; 32];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .expect("failed to read from the OS random number generator");
    hex_encode(&bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Render the `auth gen-key` output: the export line plus ready-to-paste
/// client snippets.
pub fn format_auth_output(token: &str) -> String {
    format!(
        "export COMPUTER_USE_AUTH_KEY={token}\n\n\
         # curl\n\
         curl -H \"Authorization: Bearer {token}\" http://localhost:8000/mcp\n\n\
         # .cursor/mcp.json\n\
         {{\n  \"mcpServers\": {{\n    \"computer-use\": {{\n      \"url\": \"http://localhost:8000/mcp\",\n      \"headers\": {{\n        \"Authorization\": \"Bearer {token}\"\n      }}\n    }}\n  }}\n}}\n\n\
         # claude mcp add\n\
         claude mcp add --transport http --header \"Authorization: Bearer {token}\" computer-use http://localhost:8000/mcp\n\n\
         # mcp-remote\n\
         mcp-remote --header \"Authorization: Bearer {token}\" http://localhost:8000/mcp\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::path::Path;

    #[test]
    fn clap_parses_all_flags() {
        let cli = Cli::parse_from([
            "computer-use",
            "--transport",
            "http",
            "--host",
            "0.0.0.0",
            "--port",
            "8080",
            "--auth-key",
            "k",
            "--ip-allowlist",
            "10.0.0.0/8",
            "--cors-origins",
            "https://a.example",
            "--allow-tools",
            "read",
            "--block-tools",
            "type",
            "--tls-cert",
            "/c.pem",
            "--tls-key",
            "/k.pem",
            "--allow-insecure-remote",
            "--config",
            "/tmp/x.toml",
        ]);
        assert_eq!(cli.transport, Transport::Http);
        assert_eq!(cli.host.as_deref(), Some("0.0.0.0"));
        assert_eq!(cli.port, Some(8080));
        assert_eq!(cli.auth_key.as_deref(), Some("k"));
        assert_eq!(cli.ip_allowlist.as_deref(), Some("10.0.0.0/8"));
        assert_eq!(cli.cors_origins.as_deref(), Some("https://a.example"));
        assert_eq!(cli.allow_tools.as_deref(), Some("read"));
        assert_eq!(cli.block_tools.as_deref(), Some("type"));
        assert_eq!(cli.tls_cert.as_deref(), Some(Path::new("/c.pem")));
        assert_eq!(cli.tls_key.as_deref(), Some(Path::new("/k.pem")));
        assert!(cli.allow_insecure_remote);
        assert_eq!(cli.config.as_deref(), Some(Path::new("/tmp/x.toml")));
        assert!(cli.command.is_none());
    }

    #[test]
    fn clap_transport_defaults_to_stdio() {
        let cli = Cli::parse_from(["computer-use"]);
        assert_eq!(cli.transport, Transport::Stdio);
        assert!(cli.command.is_none());
    }

    #[test]
    fn clap_parses_auth_gen_key_subcommand() {
        let cli = Cli::parse_from(["computer-use", "auth", "gen-key"]);
        assert!(matches!(
            cli.command,
            Some(Command::Auth {
                cmd: AuthArgs::GenKey {
                    write_config: false
                }
            })
        ));
    }

    #[test]
    fn clap_parses_auth_gen_key_with_write_config() {
        let cli = Cli::parse_from(["computer-use", "auth", "gen-key", "--write-config"]);
        assert!(matches!(
            cli.command,
            Some(Command::Auth {
                cmd: AuthArgs::GenKey { write_config: true }
            })
        ));
    }

    #[test]
    fn gen_key_output_shape() {
        let out = format_auth_output("aabbccdd");
        assert!(
            out.contains("export COMPUTER_USE_AUTH_KEY=aabbccdd"),
            "out: {out}"
        );
        assert!(out.contains("Authorization: Bearer aabbccdd"), "out: {out}");
        assert!(out.contains("curl"), "out: {out}");
        assert!(out.contains(".cursor/mcp.json"), "out: {out}");
        assert!(out.contains("claude mcp add"), "out: {out}");
        assert!(out.contains("mcp-remote"), "out: {out}");
    }

    #[test]
    fn generated_token_is_64_hex_chars() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "token: {token}"
        );
    }

    #[test]
    fn generated_tokens_differ() {
        assert_ne!(generate_token(), generate_token());
    }
}
