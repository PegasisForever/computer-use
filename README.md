# computer-use

MCP server providing computer use capabilities (mouse, keyboard, screenshots, screen recording) on Linux/X11.

Communicates over stdio using the [MCP protocol](https://modelcontextprotocol.io/).

## Requirements

- Linux with X11 at 1920x1080 resolution
- `xdotool`, `scrot`, `ffmpeg` installed
- CPU with AVX2 support

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
curl -L -o computer-use \
  https://github.com/PegasisForever/computer-use/releases/latest/download/computer-use-linux-x86_64
chmod +x computer-use
sudo mv computer-use /usr/local/bin/
```

2. Add the MCP server to your Claude Code settings (`~/.claude/settings.json`):

```json
{
  "mcpServers": {
    "computer-use": {
      "command": "/usr/local/bin/computer-use"
    }
  }
}
```

3. Restart Claude Code. The computer use tools will be available automatically.

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
