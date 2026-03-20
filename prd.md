# Computer Use MCP Server

## Overview

An MCP server built in Rust that provides computer use capabilities (mouse, keyboard, screenshots) and intelligent screen recording. Communicates over stdio using the [Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk).

## Constraints

- Linux only, X11 only
- Fixed 1920x1080 resolution — the server must check on startup and quit if the display resolution does not match
- All coordinates received from the AI model are in 1456x819 space and must be scaled up to 1920x1080 before execution
- All screenshots returned to the AI model are captured at 1920x1080 and resized down to 1456x819

## Tools

### 1. Mouse & Keyboard Actions

All mouse/keyboard tools perform their action, wait 1s, then take a screenshot (resized to 1456x819) and return it — unless otherwise specified below.

All click actions accept optional `x, y` parameters. If provided, smoothly move the cursor to that position first, then click.

#### `left_click(x?, y?)`

Special multi-screenshot flow:

1. Move cursor to coordinates (if provided)
2. Press down left mouse button
3. Wait 0.5s
4. Capture screenshot 1 (pressed-down state)
5. Release left mouse button
6. Wait 0.25s
7. Capture screenshot 2 (loading state)
8. Wait 1s
9. Capture screenshot 3 (loaded state)
10. Return all 3 screenshots

#### `left_double_click(x?, y?)`

Double-click the left mouse button.

#### `right_click(x?, y?)`

Click the right mouse button.

#### `middle_click(x?, y?)`

Click the middle mouse button.

#### `mouse_move(x, y)`

Smoothly move the mouse from its current position to the target `(x, y)` using ease-in-out interpolation.

#### `scroll(x?, y?, amount)`

Scroll at the current or specified position. Positive `amount` scrolls down, negative scrolls up. If `x, y` are provided, smoothly move the cursor there first. The scroll itself is also performed smoothly (incremental steps rather than a single jump).

#### `screenshot()`

Take a screenshot and return it. No action performed beforehand.

#### `key(keys)`

Press a key or key combination (e.g. `ctrl+c`, `Return`, `alt+Tab`).

#### `type(text)`

Type a string of text on the keyboard.

### 2. Screen Recording Actions

Records X264-encoded 1080p video at 15 fps.

#### `start_recording()`

Begin recording the screen. The recording is saved to a randomly generated file in `/tmp/`.

#### `stop_recording()`

Stop the current recording, finalize the output file, and return the file path.

#### `add_recording_marker(title, description)`

Insert a marker scene into the recording: a white 1080p frame with the provided title and description text rendered on it. 45 copies of this frame (3 seconds at 15 fps) are inserted into the output stream. Marker frames bypass the deduplication logic entirely — they are injected directly into the output ffmpeg encoder.

### Recording Architecture

The recording pipeline is real-time, not post-processing:

```
ffmpeg screen capture -> raw RGB stdout -> Rust program -> ffmpeg encode to MP4
```

The Rust program sits in the middle and performs **real-time frame deduplication**:

- Compare each incoming frame against the previous frame using SIMD operations
- A frame is considered "different" if it exceeds a similarity threshold
- **Keep**: all frames that differ from the previous frame ("moving frames")
- **Keep**: any frame within 0.5s (approximately 7-8 frames at 15 fps) before or after a moving frame
- **Keep**: any frame within 2s (approximately 30 frames at 15 fps) before or after a marker frame
- **Discard**: all other still/duplicate frames

This produces a final video shorter than the actual recorded duration, containing only the interesting parts with brief context around transitions. The Rust program must buffer a small window of frames to implement the look-ahead/look-behind logic and stream results in real time. The look-behind buffer must be large enough to cover the 2s marker window.

## Code Structure

All magic numbers and tunable constants (resolution, scaled resolution, FPS, wait durations, dedup threshold, buffer window, marker frame count, etc.) must be extracted into a `config.rs` module.

## Implementation Assumptions

The following assumptions were made during implementation where the PRD was ambiguous:

- **Screenshot tool**: `ffmpeg` x11grab is used to capture a single frame as PNG to stdout. No temp files or extra CLI dependencies beyond ffmpeg.
- **Mouse/keyboard input**: `xdotool` is used for all mouse and keyboard operations. It must be installed.
- **Resolution check**: `xrandr` is used to verify the display resolution on startup.
- **Smooth mouse movement**: Uses quadratic ease-in-out interpolation over 20 steps across 300ms. These values are configurable in `config.rs`.
- **Smooth scroll**: Each scroll unit is a single `xdotool click 4/5` (mouse button scroll events) with a 50ms delay between steps.
- **Frame comparison threshold**: Average absolute byte difference per byte > 0.5 is considered "different". This is a conservative threshold that detects meaningful visual changes while ignoring compression artifacts or minor rendering differences.
- **SIMD frame comparison**: Uses `std::simd` portable SIMD (nightly) with `Simd<u8, 32>` (256-bit AVX2 registers). The binary is compiled with `-C target-feature=+avx2` globally and requires an AVX2-capable CPU.
- **Marker frame text rendering**: Marker frames are rendered via `resvg` — an SVG with the title (72px bold, centered) and description (36px, centered) is built dynamically with word wrapping, then rasterized to raw RGB pixels. System fonts are loaded via `fontdb`. Falls back to a solid white frame if SVG rendering fails.
- **Recording termination**: When `stop_recording` is called, the capture ffmpeg process is killed after the pipeline loop exits. Any frames still buffered in the look-behind window at the end of recording are discarded unless they fall within 2s of a marker frame, in which case they are kept.
- **Keyboard type delay**: A 12ms inter-character delay is used with `xdotool type` to avoid dropped characters.
- **Logging**: Tracing output goes to stderr (not stdout, which is used for MCP stdio transport).
- **X11 DISPLAY**: Defaults to `:0` if the `DISPLAY` environment variable is not set.
