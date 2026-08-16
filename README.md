# computer-use

MCP server providing computer use capabilities (mouse, keyboard, screenshots, screen recording) on Linux/X11.

Communicates over stdio using the [MCP protocol](https://modelcontextprotocol.io/).

## Requirements

- Linux with X11 at 1920x1080 resolution
- `xdotool` and `ffmpeg` installed
- CPU with at least SSE2 (AVX2 optional — the SIMD recording path retargets automatically; no forced feature)

## Tools

| Tool | Description |
|---|---|
| `left_click(x?, y?)` | Left click with 3-screenshot flow (pressed, loading, loaded) |
| `left_double_click(x?, y?)` | Double-click the left mouse button |
| `right_click(x?, y?)` | Right-click |
| `middle_click(x?, y?)` | Middle-click |
| `mouse_move(x, y)` | Smooth mouse move with ease-in-out interpolation |
| `scroll(x?, y?, amount)` | Smooth scroll (positive = down, negative = up) |
| `screenshot()` | Capture screenshot |
| `key(keys)` | Press key combo (e.g. `ctrl+c`, `Return`, `alt+Tab`) |
| `type(text)` | Type text string |
| `start_recording()` | Begin screen recording with frame deduplication |
| `stop_recording()` | Stop recording and return file path |
| `add_recording_marker(title, description)` | Insert a 3-second marker scene into the recording |

All coordinates are in 1456x819 space (scaled to 1920x1080 internally).

## Usage with Claude Code

1. Download the latest release binary:

```bash
mkdir -p ~/.local/bin
curl -L -o ~/.local/bin/computer-use \
  https://github.com/PegasisForever/computer-use/releases/latest/download/computer-use-linux-x86_64
chmod +x ~/.local/bin/computer-use
```

2. Add the MCP server to your Claude Code settings (`~/.claude/settings.json`) (note: you need to use full path here):

```json
{
  "mcpServers": {
    "computer-use": {
      "command": "/home/YOUR_USER/.local/bin/computer-use"
    }
  }
}
```

3. Restart Claude Code. The computer use tools will be available automatically.

## Remote access & authentication

By default the server talks over stdio, exactly as before. To serve over HTTP instead, pass `--transport http`:

```bash
computer-use --transport http
```

This starts a Streamable HTTP server (via axum) for use from another machine or an HTTP-capable MCP client. Generate an auth key with `auth gen-key`, then start the server with it:

```bash
computer-use auth gen-key
computer-use --transport http --auth-key 'your-generated-key'
```

Clients send the key in the `Authorization: Bearer` header. Never put it in the URL.

Startup rules:

- Loopback binds work without an auth key.
- A non-loopback bind requires an auth key AND either TLS or `--allow-insecure-remote`.

TLS is optional, behind a cargo feature `tls` off by default; build with `cargo +nightly build --release --features tls`.

See [SECURITY.md](SECURITY.md) for the full threat model.

### CLI flags

| Flag | Description |
|---|---|
| `--transport stdio\|http` | Transport mode. Default `stdio`. |
| `--host` | Address to bind. |
| `--port` | Port to listen on. |
| `--auth-key` | Bearer token required from clients. |
| `--ip-allowlist` | Comma-separated CIDRs allowed to connect. Empty allows all. |
| `--cors-origins` | Comma-separated allowed origins for CORS. |
| `--allow-tools` | Comma-separated tool allow list. |
| `--block-tools` | Comma-separated tool deny list. |
| `--tls-cert` | TLS certificate file (requires the `tls` feature). |
| `--tls-key` | TLS private key file (requires the `tls` feature). |
| `--allow-insecure-remote` | Allow non-loopback binds without TLS. |
| `--config <path>` | Path to a config.toml file (HTTP mode only). |

### Environment variables

Every flag has a `COMPUTER_USE_*` environment variable. Precedence: CLI flags > environment variables > `config.toml` > defaults.

| Variable | Equivalent flag |
|---|---|
| `COMPUTER_USE_AUTH_KEY` | `--auth-key` |
| `COMPUTER_USE_IP_ALLOWLIST` | `--ip-allowlist` |
| `COMPUTER_USE_CORS_ORIGINS` | `--cors-origins` |
| `COMPUTER_USE_TOOL_ALLOW` | `--allow-tools` |
| `COMPUTER_USE_TOOL_DENY` | `--block-tools` |
| `COMPUTER_USE_LISTEN_ADDR` | `--host` and `--port` |
| `COMPUTER_USE_TLS_CERT` | `--tls-cert` |
| `COMPUTER_USE_TLS_KEY` | `--tls-key` |
| `COMPUTER_USE_ALLOW_INSECURE_REMOTE` | `--allow-insecure-remote` |

### config.toml

A config file is optional and read only in HTTP mode. Example:

```toml
[auth]
token = "your-token"

[http]
bind = "127.0.0.1:3000"

[http.tls]
cert = "/path/to/cert.pem"
key = "/path/to/key.pem"
```

### Security posture

The server ships with a documented threat model covering three attacker classes: browser-based DNS rebinding, local or LAN peers, and compromised agents. Defenses include loopback-only default binds, Origin and Host validation, Bearer authentication, an optional IP allowlist, and tool allow and deny lists enforced on both transports. The honest line: this authenticates the channel; a compromised agent can still type passwords and click destructive buttons. Read [SECURITY.md](SECURITY.md) for details.

## Testing locally with MCP Inspector

[MCP Inspector](https://modelcontextprotocol.io/docs/tools/inspector) lets you interact with MCP servers directly in the browser.

1. Build the server:

```bash
cargo +nightly build --release
```

2. Launch the inspector, pointing it at the binary:

```bash
npx @modelcontextprotocol/inspector target/release/computer-use
```

3. Open the URL printed by the inspector (usually `http://localhost:6274`). You can browse the available tools, call them with parameters, and see the returned screenshots.

## Building from source

```bash
# Requires Rust nightly (for std::simd)
cargo +nightly build --release
```

The binary is at `target/release/computer-use`.

## Creating a release

```bash
./release.sh
```

This compiles a production binary, strips debug symbols, and creates a GitHub release tagged with the version from `Cargo.toml`.
