//! Configuration: hardcoded constants plus the resolved runtime `Config`.
//!
//! Precedence: CLI > env (`COMPUTER_USE_*`) > `config.toml` (HTTP mode only)
//! > defaults.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Native display resolution (must match actual X11 display).
pub const DISPLAY_WIDTH: u32 = 1920;
pub const DISPLAY_HEIGHT: u32 = 1080;

/// Scaled resolution for AI model communication.
pub const SCALED_WIDTH: u32 = 1456;
pub const SCALED_HEIGHT: u32 = 819;

/// Recording frames per second.
pub const RECORDING_FPS: u32 = 15;

/// Wait time after actions before taking screenshot (seconds).
pub const ACTION_WAIT_SECS: f64 = 1.0;

/// left_click press-down wait before first screenshot (seconds).
pub const LEFT_CLICK_PRESS_WAIT_SECS: f64 = 0.5;

/// left_click wait after release before second screenshot (seconds).
pub const LEFT_CLICK_RELEASE_WAIT_SECS: f64 = 0.25;

/// Number of interpolation steps for smooth mouse movement.
pub const MOUSE_MOVE_STEPS: u32 = 10;

/// Total duration of smooth mouse movement (milliseconds).
pub const MOUSE_MOVE_DURATION_MS: u64 = 100;

/// Delay between individual scroll steps (milliseconds).
pub const SCROLL_STEP_DELAY_MS: u64 = 25;

/// Frame deduplication threshold: average absolute byte difference per byte.
pub const DEDUP_THRESHOLD: f64 = 0.25;

/// Deduplication look-ahead/behind window in frames (~0.25s at 15fps).
pub const DEDUP_LOOK_WINDOW: usize = 4;

/// Deduplication look-ahead/behind window around marker frames (~2s at 15fps).
pub const MARKER_LOOK_WINDOW: usize = 30;

/// Number of marker frames inserted (3 seconds at 15fps).
pub const MARKER_FRAME_COUNT: usize = 45;

/// Directory for recording output files.
pub const RECORDING_DIR: &str = "/tmp";

/// Environment variable prefix for all `COMPUTER_USE_*` overrides.
pub const ENV_PREFIX: &str = "COMPUTER_USE_";

/// Default HTTP bind host.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Default HTTP bind port.
pub const DEFAULT_PORT: u16 = 8000;

/// Transport mode. stdio is the default; http is the optional Streamable
/// HTTP server mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum Transport {
    #[default]
    Stdio,
    Http,
}

/// Environment snapshot: full `COMPUTER_USE_*` variable names → values.
pub type EnvMap = BTreeMap<String, String>;

/// Raw CLI-parsed values before precedence is applied (`None` = flag absent).
#[derive(Debug, Clone, Default)]
pub struct CliValues {
    pub transport: Option<Transport>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub auth_key: Option<String>,
    pub ip_allowlist: Option<String>,
    pub cors_origins: Option<String>,
    pub tool_allow: Option<String>,
    pub tool_deny: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub allow_insecure_remote: bool,
    pub config_path: Option<String>,
}

/// Fully resolved runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub transport: Transport,
    pub host: String,
    pub port: u16,
    pub auth_key: Option<String>,
    pub ip_allowlist: Vec<String>,
    pub cors_origins: Vec<String>,
    pub tool_allow: Vec<String>,
    pub tool_deny: Vec<String>,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub allow_insecure_remote: bool,
    pub config_path: Option<PathBuf>,
}

/// On-disk `config.toml` shape: exactly three sections
/// (`[auth] token`, `[http] bind`, `[http.tls] cert`/`key`).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    auth: Option<FileAuth>,
    http: Option<FileHttp>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAuth {
    token: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileHttp {
    bind: Option<String>,
    tls: Option<FileTls>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileTls {
    cert: Option<String>,
    key: Option<String>,
}

impl Config {
    /// Resolve the configuration for a parsed CLI, applying
    /// CLI > env > config.toml (HTTP mode only) > defaults.
    pub fn load(cli: &crate::cli::Cli) -> Result<Self> {
        let values = CliValues::from(cli);
        let env = current_env();
        let file = if values.transport == Some(Transport::Http) {
            load_config_file(values.config_path.as_deref())?
        } else {
            None
        };
        Self::from_sources(&values, &env, file.as_deref())
    }

    /// Pure precedence resolution over explicit inputs. Testable without
    /// mutating the real process environment.
    ///
    /// `file` is the `config.toml` contents, if one should be applied.
    pub fn from_sources(cli: &CliValues, env: &EnvMap, file: Option<&str>) -> Result<Self> {
        let file_cfg = match file {
            Some(contents) => {
                Some(toml::from_str::<FileConfig>(contents).context("invalid config.toml")?)
            }
            None => None,
        };
        let file = file_cfg.as_ref();

        let env_bind = match env_get(env, "LISTEN_ADDR") {
            Some(addr) => Some(split_bind(addr)?),
            None => None,
        };
        let file_bind = match file
            .and_then(|f| f.http.as_ref())
            .and_then(|h| h.bind.as_deref())
        {
            Some(addr) => Some(split_bind(addr)?),
            None => None,
        };

        let host = cli
            .host
            .clone()
            .or_else(|| env_bind.as_ref().map(|b| b.0.clone()))
            .or_else(|| file_bind.as_ref().map(|b| b.0.clone()))
            .unwrap_or_else(|| DEFAULT_HOST.to_string());

        let port = cli
            .port
            .or_else(|| env_bind.as_ref().map(|b| b.1))
            .or_else(|| file_bind.as_ref().map(|b| b.1))
            .unwrap_or(DEFAULT_PORT);

        let auth_key = cli
            .auth_key
            .clone()
            .or_else(|| env_get(env, "AUTH_KEY").map(String::from))
            .or_else(|| {
                file.and_then(|f| f.auth.as_ref())
                    .and_then(|a| a.token.clone())
            });

        let ip_allowlist = csv_list(
            cli.ip_allowlist
                .as_deref()
                .or_else(|| env_get(env, "IP_ALLOWLIST")),
        );
        let cors_origins = csv_list(
            cli.cors_origins
                .as_deref()
                .or_else(|| env_get(env, "CORS_ORIGINS")),
        );
        let tool_allow = csv_list(
            cli.tool_allow
                .as_deref()
                .or_else(|| env_get(env, "TOOL_ALLOW")),
        );
        let tool_deny = csv_list(
            cli.tool_deny
                .as_deref()
                .or_else(|| env_get(env, "TOOL_DENY")),
        );

        let tls_cert = cli
            .tls_cert
            .clone()
            .or_else(|| env_get(env, "TLS_CERT").map(String::from))
            .or_else(|| {
                file.and_then(|f| f.http.as_ref())
                    .and_then(|h| h.tls.as_ref())
                    .and_then(|t| t.cert.clone())
            })
            .map(PathBuf::from);

        let tls_key = cli
            .tls_key
            .clone()
            .or_else(|| env_get(env, "TLS_KEY").map(String::from))
            .or_else(|| {
                file.and_then(|f| f.http.as_ref())
                    .and_then(|h| h.tls.as_ref())
                    .and_then(|t| t.key.clone())
            })
            .map(PathBuf::from);

        let allow_insecure_remote = cli.allow_insecure_remote
            || env_get(env, "ALLOW_INSECURE_REMOTE")
                .map(is_truthy)
                .unwrap_or(false);

        Ok(Self {
            transport: cli.transport.unwrap_or_default(),
            host,
            port,
            auth_key,
            ip_allowlist,
            cors_origins,
            tool_allow,
            tool_deny,
            tls_cert,
            tls_key,
            allow_insecure_remote,
            config_path: cli.config_path.clone().map(PathBuf::from),
        })
    }
}

/// Look up a `COMPUTER_USE_<name>` variable in an env map.
fn env_get<'a>(env: &'a EnvMap, name: &str) -> Option<&'a str> {
    env.get(&format!("{ENV_PREFIX}{name}")).map(String::as_str)
}

/// Parse a `host:port` bind address, supporting bracketed IPv6 (`[::1]:8000`).
fn split_bind(addr: &str) -> Result<(String, u16)> {
    let (host, port) = if let Some(rest) = addr.strip_prefix('[') {
        let (host, port) = rest
            .split_once("]:")
            .with_context(|| format!("invalid bind address {addr:?}: expected [ipv6]:port"))?;
        (format!("[{host}]"), port)
    } else {
        let (host, port) = addr
            .rsplit_once(':')
            .with_context(|| format!("invalid bind address {addr:?}: expected host:port"))?;
        (host.to_string(), port)
    };
    if host.is_empty() {
        bail!("invalid bind address {addr:?}: empty host");
    }
    let port = port
        .parse::<u16>()
        .with_context(|| format!("invalid bind address {addr:?}: bad port"))?;
    Ok((host, port))
}

/// Split a comma-separated list, trimming whitespace and skipping empties.
fn csv_list(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

/// `true`/`1`/`yes` (case-insensitive) are truthy.
fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes"
    )
}

/// Read the process `COMPUTER_USE_*` environment variables.
pub fn current_env() -> EnvMap {
    std::env::vars()
        .filter(|(k, _)| k.starts_with(ENV_PREFIX))
        .collect()
}

/// Default config file location: `~/.config/computer-use/config.toml`.
pub fn default_config_path() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join(".config")
        .join("computer-use")
        .join("config.toml")
}

/// Write the three-section `config.toml` with the given auth token.
pub fn write_config_file(path: &Path, token: &str) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let contents =
        format!("[auth]\ntoken = {token:?}\n\n[http]\nbind = \"{DEFAULT_HOST}:{DEFAULT_PORT}\"\n");
    std::fs::write(path, contents)
        .with_context(|| format!("failed to write config file {}", path.display()))
}

/// Read an explicitly given or default config file. Returns `None` when no
/// default file exists.
fn load_config_file(explicit: Option<&str>) -> Result<Option<String>> {
    match explicit {
        Some(path) => {
            let p = Path::new(path);
            let contents = std::fs::read_to_string(p)
                .with_context(|| format!("failed to read config file {}", p.display()))?;
            Ok(Some(contents))
        }
        None => {
            let path = default_config_path();
            if path.is_file() {
                let contents = std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read config file {}", path.display()))?;
                Ok(Some(contents))
            } else {
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(entries: &[(&str, &str)]) -> EnvMap {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn build(cli: CliValues, env: &[(&str, &str)], file: Option<&str>) -> Result<Config> {
        Config::from_sources(&cli, &env_map(env), file)
    }

    fn cli() -> CliValues {
        CliValues::default()
    }

    #[test]
    fn cli_wins_over_env() {
        let mut c = cli();
        c.auth_key = Some("cli-key".into());
        let cfg = build(c, &[("COMPUTER_USE_AUTH_KEY", "env-key")], None).unwrap();
        assert_eq!(cfg.auth_key.as_deref(), Some("cli-key"));
    }

    #[test]
    fn env_wins_over_config_file() {
        let cfg = build(
            cli(),
            &[("COMPUTER_USE_AUTH_KEY", "env-key")],
            Some("[auth]\ntoken = \"file-key\"\n"),
        )
        .unwrap();
        assert_eq!(cfg.auth_key.as_deref(), Some("env-key"));
    }

    #[test]
    fn config_file_wins_over_defaults() {
        let cfg = build(cli(), &[], Some("[http]\nbind = \"0.0.0.0:9000\"\n")).unwrap();
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 9000);
    }

    #[test]
    fn defaults_used_when_nothing_set() {
        let cfg = build(cli(), &[], None).unwrap();
        assert_eq!(cfg.transport, Transport::Stdio);
        assert_eq!(cfg.host, DEFAULT_HOST);
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert_eq!(cfg.auth_key, None);
        assert!(cfg.ip_allowlist.is_empty());
        assert!(cfg.cors_origins.is_empty());
        assert!(cfg.tool_allow.is_empty());
        assert!(cfg.tool_deny.is_empty());
        assert_eq!(cfg.tls_cert, None);
        assert_eq!(cfg.tls_key, None);
        assert!(!cfg.allow_insecure_remote);
        assert_eq!(cfg.config_path, None);
    }

    #[test]
    fn cli_transport_is_respected() {
        let mut c = cli();
        c.transport = Some(Transport::Http);
        let cfg = build(c, &[], None).unwrap();
        assert_eq!(cfg.transport, Transport::Http);
    }

    #[test]
    fn env_listen_addr_sets_host_and_port() {
        let cfg = build(cli(), &[("COMPUTER_USE_LISTEN_ADDR", "0.0.0.0:9999")], None).unwrap();
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 9999);
    }

    #[test]
    fn cli_port_wins_over_env_listen_addr() {
        let mut c = cli();
        c.port = Some(7000);
        let cfg = build(c, &[("COMPUTER_USE_LISTEN_ADDR", "0.0.0.0:9999")], None).unwrap();
        assert_eq!(cfg.port, 7000);
        assert_eq!(cfg.host, "0.0.0.0");
    }

    #[test]
    fn env_listen_addr_ipv6_bracketed() {
        let cfg = build(cli(), &[("COMPUTER_USE_LISTEN_ADDR", "[::1]:8080")], None).unwrap();
        assert_eq!(cfg.host, "[::1]");
        assert_eq!(cfg.port, 8080);
    }

    #[test]
    fn env_csv_lists_trim_and_skip_empty() {
        let cfg = build(
            cli(),
            &[(
                "COMPUTER_USE_IP_ALLOWLIST",
                " 10.0.0.0/8 ,,192.168.1.0/24 , ",
            )],
            None,
        )
        .unwrap();
        assert_eq!(cfg.ip_allowlist, vec!["10.0.0.0/8", "192.168.1.0/24"]);
    }

    #[test]
    fn cli_csv_lists_are_parsed() {
        let mut c = cli();
        c.tool_allow = Some(" read,  click ".into());
        let cfg = build(c, &[], None).unwrap();
        assert_eq!(cfg.tool_allow, vec!["read", "click"]);
    }

    #[test]
    fn insecure_remote_env_truthy_values() {
        for value in ["true", "1", "yes"] {
            let cfg = build(
                cli(),
                &[("COMPUTER_USE_ALLOW_INSECURE_REMOTE", value)],
                None,
            )
            .unwrap();
            assert!(
                cfg.allow_insecure_remote,
                "env value {value:?} must be truthy"
            );
        }
    }

    #[test]
    fn insecure_remote_env_other_values_false() {
        let cfg = build(
            cli(),
            &[("COMPUTER_USE_ALLOW_INSECURE_REMOTE", "nope")],
            None,
        )
        .unwrap();
        assert!(!cfg.allow_insecure_remote);
    }

    #[test]
    fn config_file_reads_all_three_sections() {
        let file = "[auth]\ntoken = \"file-key\"\n\n[http]\nbind = \"127.0.0.1:9000\"\n\n[http.tls]\ncert = \"/tmp/cert.pem\"\nkey = \"/tmp/key.pem\"\n";
        let cfg = build(cli(), &[], Some(file)).unwrap();
        assert_eq!(cfg.auth_key.as_deref(), Some("file-key"));
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 9000);
        assert_eq!(cfg.tls_cert.as_deref(), Some(Path::new("/tmp/cert.pem")));
        assert_eq!(cfg.tls_key.as_deref(), Some(Path::new("/tmp/key.pem")));
    }

    #[test]
    fn config_file_tls_overridden_by_env_and_cli() {
        let file = "[http.tls]\ncert = \"/file/cert.pem\"\nkey = \"/file/key.pem\"\n";
        let cfg = build(
            cli(),
            &[("COMPUTER_USE_TLS_CERT", "/env/cert.pem")],
            Some(file),
        )
        .unwrap();
        assert_eq!(cfg.tls_cert.as_deref(), Some(Path::new("/env/cert.pem")));
        assert_eq!(cfg.tls_key.as_deref(), Some(Path::new("/file/key.pem")));

        let mut c = cli();
        c.tls_cert = Some("/cli/cert.pem".into());
        let cfg = build(c, &[("COMPUTER_USE_TLS_CERT", "/env/cert.pem")], Some(file)).unwrap();
        assert_eq!(cfg.tls_cert.as_deref(), Some(Path::new("/cli/cert.pem")));
    }

    #[test]
    fn malformed_config_file_is_error() {
        let file = "not = [valid toml";
        assert!(build(cli(), &[], Some(file)).is_err());
    }

    #[test]
    fn config_file_with_unknown_sections_is_error() {
        let file = "[server]\nfoo = \"bar\"\n";
        assert!(build(cli(), &[], Some(file)).is_err());
    }

    #[test]
    fn malformed_env_listen_addr_is_error() {
        assert!(
            build(
                cli(),
                &[("COMPUTER_USE_LISTEN_ADDR", "not-an-address")],
                None
            )
            .is_err()
        );
    }

    #[test]
    fn cli_config_path_is_recorded() {
        let mut c = cli();
        c.config_path = Some("/tmp/x.toml".into());
        let cfg = build(c, &[], None).unwrap();
        assert_eq!(cfg.config_path.as_deref(), Some(Path::new("/tmp/x.toml")));
    }

    #[test]
    fn write_config_file_roundtrips() {
        let dir = std::env::temp_dir().join(format!("cu-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        write_config_file(&path, "tok123").unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let cfg =
            Config::from_sources(&CliValues::default(), &EnvMap::new(), Some(&contents)).unwrap();
        assert_eq!(cfg.auth_key.as_deref(), Some("tok123"));
        assert_eq!(cfg.host, DEFAULT_HOST);
        assert_eq!(cfg.port, DEFAULT_PORT);
        std::fs::remove_dir_all(&dir).ok();
    }
}
