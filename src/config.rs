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
pub const MOUSE_MOVE_STEPS: u32 = 20;

/// Total duration of smooth mouse movement (milliseconds).
pub const MOUSE_MOVE_DURATION_MS: u64 = 300;

/// Delay between individual scroll steps (milliseconds).
pub const SCROLL_STEP_DELAY_MS: u64 = 50;

/// Frame deduplication threshold: average absolute byte difference per byte.
pub const DEDUP_THRESHOLD: f64 = 0.5;

/// Deduplication look-ahead/behind window in frames (~0.5s at 15fps).
pub const DEDUP_LOOK_WINDOW: usize = 8;

/// Number of marker frames inserted (3 seconds at 15fps).
pub const MARKER_FRAME_COUNT: usize = 45;

/// Directory for recording output files.
pub const RECORDING_DIR: &str = "/tmp";
