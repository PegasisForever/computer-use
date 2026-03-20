use crate::config::*;
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Frame comparison — explicit SIMD via std::simd (AVX2)
// ---------------------------------------------------------------------------

/// Compare two frames using sum of absolute differences with explicit
/// `std::simd` SIMD vectors. Compiled with AVX2 enabled globally so
/// `Simd<u8, 32>` maps directly to 256-bit YMM registers.
#[inline]
pub fn frames_are_different(a: &[u8], b: &[u8], threshold: f64) -> bool {
    use std::simd::prelude::*;

    debug_assert_eq!(a.len(), b.len());
    let len = a.len();
    if len == 0 {
        return false;
    }

    let mut total: u64 = 0;
    let chunks = len / 32;

    for i in 0..chunks {
        let off = i * 32;
        let va = Simd::<u8, 32>::from_slice(&a[off..]);
        let vb = Simd::<u8, 32>::from_slice(&b[off..]);

        // Branchless absolute difference: max(a,b) - min(a,b)
        let diff = va.simd_max(vb) - va.simd_min(vb);

        // Widen u8x32 → u16x32 so reduce_sum can't overflow
        // (max per-chunk sum = 32 * 255 = 8160, fits in u16).
        let wide: Simd<u16, 32> = diff.cast();
        total += wide.reduce_sum() as u64;
    }

    // Remainder bytes
    for i in (chunks * 32)..len {
        total += a[i].abs_diff(b[i]) as u64;
    }

    let avg_diff = total as f64 / len as f64;
    avg_diff > threshold
}

// ---------------------------------------------------------------------------
// Frame deduplicator (pure logic, fully testable)
// ---------------------------------------------------------------------------

/// Real-time frame deduplicator.
///
/// A frame is "moving" if it differs from the previous frame beyond a threshold.
/// The deduplicator keeps:
///   - All moving frames
///   - Up to `look_window` still frames before a moving frame (look-behind)
///   - Up to `look_window` still frames after a moving frame (look-ahead)
///
/// Internally it maintains a look-behind buffer and a countdown for look-ahead.
pub struct FrameDeduplicator {
    threshold: f64,
    look_window: usize,
    buffer: VecDeque<Vec<u8>>,
    prev_frame: Option<Vec<u8>>,
    countdown: usize,
}

impl FrameDeduplicator {
    pub fn new(threshold: f64, look_window: usize) -> Self {
        Self {
            threshold,
            look_window,
            buffer: VecDeque::new(),
            prev_frame: None,
            countdown: 0,
        }
    }

    /// Feed a frame into the deduplicator. Returns the frames that should be
    /// written to the encoder at this point (may be 0, 1, or several).
    pub fn push_frame(&mut self, frame: Vec<u8>) -> Vec<Vec<u8>> {
        let is_moving = match &self.prev_frame {
            Some(prev) => frames_are_different(prev, &frame, self.threshold),
            None => true, // first frame is always considered moving
        };

        self.prev_frame = Some(frame.clone());
        let mut output = Vec::new();

        if is_moving {
            // Flush look-behind buffer (all within look_window of this moving frame)
            output.extend(self.buffer.drain(..));
            output.push(frame);
            self.countdown = self.look_window;
        } else if self.countdown > 0 {
            // Still in look-ahead window after a moving frame
            output.push(frame);
            self.countdown -= 1;
        } else {
            // Buffer for potential look-behind
            self.buffer.push_back(frame);
            if self.buffer.len() > self.look_window {
                self.buffer.pop_front();
            }
        }

        output
    }

    /// Flush at end of recording. Still frames sitting in the look-behind
    /// buffer are not adjacent to any future moving frame, so they are discarded.
    pub fn flush(&mut self) -> Vec<Vec<u8>> {
        self.buffer.clear();
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Marker frame generation (SVG text rendered via resvg)
// ---------------------------------------------------------------------------

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Word-wrap `text` into lines of at most `max_chars` characters.
fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        if current_line.is_empty() {
            current_line = word.to_string();
        } else if current_line.len() + 1 + word.len() <= max_chars {
            current_line.push(' ');
            current_line.push_str(word);
        } else {
            lines.push(current_line);
            current_line = word.to_string();
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    lines
}

/// Build an SVG document with centered title (bold, large) and description
/// (smaller) text, word-wrapped to fit the frame width.
fn build_marker_svg(width: u32, height: u32, title: &str, description: &str) -> String {
    let margin: u32 = 100;
    let available = width.saturating_sub(2 * margin).max(1);

    let title_fs: u32 = 72;
    let desc_fs: u32 = 36;
    let title_lh: u32 = 88;
    let desc_lh: u32 = 44;
    let gap: u32 = 40;

    // Estimate chars per line (avg char width ≈ 0.6 × font-size)
    let title_cpl = (available as f64 / (title_fs as f64 * 0.6)) as usize;
    let desc_cpl = (available as f64 / (desc_fs as f64 * 0.6)) as usize;

    let title_lines = wrap_text(title, title_cpl);
    let desc_lines = wrap_text(description, desc_cpl);

    let total_title = title_lines.len() as u32 * title_lh;
    let total_desc = desc_lines.len() as u32 * desc_lh;
    let total_h = total_title + gap + total_desc;

    // Vertical center; +font_size for baseline offset
    let start_y = (height.saturating_sub(total_h)) / 2 + title_fs;
    let cx = width / 2;

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">"#,
    );
    svg.push_str(r#"<rect width="100%" height="100%" fill="white"/>"#);

    for (i, line) in title_lines.iter().enumerate() {
        let y = start_y + i as u32 * title_lh;
        svg.push_str(&format!(
            r#"<text x="{cx}" y="{y}" text-anchor="middle" font-size="{title_fs}" font-weight="bold" fill="black">{}</text>"#,
            xml_escape(line),
        ));
    }

    let desc_y0 = start_y + total_title + gap;
    for (i, line) in desc_lines.iter().enumerate() {
        let y = desc_y0 + i as u32 * desc_lh;
        svg.push_str(&format!(
            "<text x=\"{cx}\" y=\"{y}\" text-anchor=\"middle\" font-size=\"{desc_fs}\" fill=\"#333333\">{}</text>",
            xml_escape(line),
        ));
    }

    svg.push_str("</svg>");
    svg
}

/// Render a marker frame: white 1080p frame with title and description text.
/// Uses resvg to rasterize an SVG with the text laid out.
/// Returns raw RGB24 bytes (width × height × 3).
pub fn generate_marker_frame(width: u32, height: u32, title: &str, description: &str) -> Vec<u8> {
    let rgb_size = (width * height * 3) as usize;
    let white_fallback = || vec![255u8; rgb_size];

    let svg_str = build_marker_svg(width, height, title, description);

    let mut opt = resvg::usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();

    let tree = match resvg::usvg::Tree::from_data(svg_str.as_bytes(), &opt) {
        Ok(t) => t,
        Err(_) => return white_fallback(),
    };

    let mut pixmap = match resvg::tiny_skia::Pixmap::new(width, height) {
        Some(p) => p,
        None => return white_fallback(),
    };

    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(&tree, resvg::tiny_skia::Transform::default(), &mut pixmap.as_mut());

    // Convert premultiplied RGBA → RGB24.
    // Background is opaque white so every pixel has alpha=255;
    // premultiplied values equal straight values.
    let rgba = pixmap.data();
    let mut rgb = Vec::with_capacity(rgb_size);
    for px in rgba.chunks_exact(4) {
        rgb.push(px[0]);
        rgb.push(px[1]);
        rgb.push(px[2]);
    }
    rgb
}

// ---------------------------------------------------------------------------
// Recording pipeline (generic over I/O for testability)
// ---------------------------------------------------------------------------

/// Messages sent to the recording pipeline task.
pub enum RecordingMessage {
    MarkerFrame { title: String, description: String },
    Stop,
}

/// Run the deduplication pipeline, reading raw RGB frames from `capture` and
/// writing kept frames to `encoder`. Also handles marker injection and stop
/// signals via `msg_rx`.
///
/// Generic over AsyncRead/AsyncWrite so tests can substitute in-memory streams.
pub async fn run_pipeline(
    capture: impl AsyncRead + Unpin + Send + 'static,
    mut encoder: impl AsyncWrite + Unpin + Send,
    mut msg_rx: mpsc::Receiver<RecordingMessage>,
    frame_width: u32,
    frame_height: u32,
    dedup_threshold: f64,
    look_window: usize,
    marker_frame_count: usize,
) -> Result<()> {
    let frame_size = (frame_width as usize) * (frame_height as usize) * 3;

    // Spawn a reader task so frame reads don't get cancelled by select!
    let (frame_tx, mut frame_rx) = mpsc::channel::<Vec<u8>>(10);

    let reader_handle: JoinHandle<()> = tokio::spawn(async move {
        let mut reader = capture;
        let mut buf = vec![0u8; frame_size];
        loop {
            match read_exact_frame(&mut reader, &mut buf).await {
                Ok(true) => {
                    let frame = std::mem::replace(&mut buf, vec![0u8; frame_size]);
                    if frame_tx.send(frame).await.is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
    });

    let mut dedup = FrameDeduplicator::new(dedup_threshold, look_window);

    loop {
        tokio::select! {
            biased;

            // Prioritize frames so we don't discard buffered data on Stop
            frame = frame_rx.recv() => {
                match frame {
                    Some(data) => {
                        let kept = dedup.push_frame(data);
                        for f in kept {
                            encoder.write_all(&f).await
                                .context("failed to write frame to encoder")?;
                        }
                    }
                    None => break, // capture ended
                }
            }

            msg = msg_rx.recv() => {
                match msg {
                    Some(RecordingMessage::MarkerFrame { title, description }) => {
                        let marker = generate_marker_frame(
                            frame_width, frame_height, &title, &description,
                        );
                        for _ in 0..marker_frame_count {
                            encoder.write_all(&marker).await
                                .context("failed to write marker frame")?;
                        }
                    }
                    Some(RecordingMessage::Stop) | None => break,
                }
            }
        }
    }

    // Flush remaining dedup buffer
    let remaining = dedup.flush();
    for f in remaining {
        encoder
            .write_all(&f)
            .await
            .context("failed to write flushed frame")?;
    }

    reader_handle.abort();
    Ok(())
}

/// Read exactly `buf.len()` bytes. Returns Ok(true) on success, Ok(false) on EOF.
async fn read_exact_frame(
    reader: &mut (impl AsyncReadExt + Unpin),
    buf: &mut [u8],
) -> Result<bool> {
    let mut total_read = 0;
    while total_read < buf.len() {
        match reader.read(&mut buf[total_read..]).await? {
            0 => return Ok(false),
            n => total_read += n,
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Recording handle (manages ffmpeg processes)
// ---------------------------------------------------------------------------

pub struct RecordingHandle {
    pub output_path: String,
    msg_tx: mpsc::Sender<RecordingMessage>,
    join_handle: JoinHandle<Result<()>>,
}

impl RecordingHandle {
    pub async fn stop(self) -> Result<String> {
        self.msg_tx.send(RecordingMessage::Stop).await.ok();
        self.join_handle
            .await
            .context("recording task panicked")??;
        Ok(self.output_path)
    }

    pub async fn add_marker(&self, title: String, description: String) -> Result<()> {
        self.msg_tx
            .send(RecordingMessage::MarkerFrame { title, description })
            .await
            .context("recording task is gone")?;
        Ok(())
    }
}

/// Start a new screen recording. Returns a handle for stopping / adding markers.
pub fn start_recording() -> Result<RecordingHandle> {
    let output_path = format!(
        "{}/recording_{}.mp4",
        RECORDING_DIR,
        rand::random::<u64>()
    );
    let output_path_clone = output_path.clone();
    let (msg_tx, msg_rx) = mpsc::channel(32);

    let join_handle = tokio::spawn(async move {
        run_recording_with_ffmpeg(&output_path_clone, msg_rx).await
    });

    Ok(RecordingHandle {
        output_path,
        msg_tx,
        join_handle,
    })
}

async fn run_recording_with_ffmpeg(
    output_path: &str,
    msg_rx: mpsc::Receiver<RecordingMessage>,
) -> Result<()> {
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());

    let mut capture = Command::new("ffmpeg")
        .args([
            "-f",
            "x11grab",
            "-video_size",
            &format!("{}x{}", DISPLAY_WIDTH, DISPLAY_HEIGHT),
            "-framerate",
            &RECORDING_FPS.to_string(),
            "-i",
            &display,
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .context("failed to start ffmpeg capture")?;

    let mut encoder = Command::new("ffmpeg")
        .args([
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-video_size",
            &format!("{}x{}", DISPLAY_WIDTH, DISPLAY_HEIGHT),
            "-framerate",
            &RECORDING_FPS.to_string(),
            "-i",
            "pipe:0",
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-pix_fmt",
            "yuv420p",
            "-y",
            output_path,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start ffmpeg encoder")?;

    let capture_stdout = capture.stdout.take().unwrap();
    let encoder_stdin = encoder.stdin.take().unwrap();

    let result = run_pipeline(
        capture_stdout,
        encoder_stdin,
        msg_rx,
        DISPLAY_WIDTH,
        DISPLAY_HEIGHT,
        DEDUP_THRESHOLD,
        DEDUP_LOOK_WINDOW,
        MARKER_FRAME_COUNT,
    )
    .await;

    let _ = capture.kill().await;
    let _ = encoder.wait().await;

    result
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    // Test dimensions: 4×4 pixels → frame_size = 48 bytes (divisible by 3)
    const TW: u32 = 4;
    const TH: u32 = 4;
    const TFS: usize = (TW as usize) * (TH as usize) * 3;

    fn make_frame(value: u8, size: usize) -> Vec<u8> {
        vec![value; size]
    }

    // --- frames_are_different ---

    #[test]
    fn test_identical_frames_not_different() {
        let a = make_frame(128, 1024);
        assert!(!frames_are_different(&a, &a, DEDUP_THRESHOLD));
    }

    #[test]
    fn test_completely_different_frames() {
        let a = make_frame(0, 1024);
        let b = make_frame(255, 1024);
        assert!(frames_are_different(&a, &b, DEDUP_THRESHOLD));
    }

    #[test]
    fn test_slightly_different_frames_below_threshold() {
        let a = make_frame(128, 10000);
        let mut b = a.clone();
        for i in 0..1 {
            b[i] = 129;
        }
        assert!(!frames_are_different(&a, &b, DEDUP_THRESHOLD));
    }

    #[test]
    fn test_empty_frames_not_different() {
        let a: Vec<u8> = vec![];
        assert!(!frames_are_different(&a, &a, DEDUP_THRESHOLD));
    }

    #[test]
    fn test_threshold_boundary() {
        let a = make_frame(100, 1024);
        let b = make_frame(101, 1024);
        assert!(frames_are_different(&a, &b, DEDUP_THRESHOLD));
    }

    // --- FrameDeduplicator ---

    #[test]
    fn test_dedup_first_frame_always_output() {
        let mut dedup = FrameDeduplicator::new(DEDUP_THRESHOLD, 3);
        let out = dedup.push_frame(make_frame(128, TFS));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn test_dedup_all_identical_drops_after_lookahead() {
        let mut dedup = FrameDeduplicator::new(DEDUP_THRESHOLD, 3);
        let frame = make_frame(128, TFS);

        assert_eq!(dedup.push_frame(frame.clone()).len(), 1);
        for _ in 0..3 {
            assert_eq!(dedup.push_frame(frame.clone()).len(), 1);
        }
        for _ in 0..5 {
            assert_eq!(dedup.push_frame(frame.clone()).len(), 0);
        }
    }

    #[test]
    fn test_dedup_all_different_all_output() {
        let mut dedup = FrameDeduplicator::new(DEDUP_THRESHOLD, 3);
        for i in 0..10u8 {
            let out = dedup.push_frame(make_frame(i * 25, TFS));
            assert!(!out.is_empty(), "frame {} should be output", i);
        }
    }

    #[test]
    fn test_dedup_look_behind_flushed_on_moving_frame() {
        let mut dedup = FrameDeduplicator::new(DEDUP_THRESHOLD, 3);

        assert_eq!(dedup.push_frame(make_frame(128, TFS)).len(), 1);
        for _ in 0..3 {
            dedup.push_frame(make_frame(128, TFS));
        }
        for _ in 0..5 {
            assert_eq!(dedup.push_frame(make_frame(128, TFS)).len(), 0);
        }
        let out = dedup.push_frame(make_frame(0, TFS));
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn test_dedup_look_ahead_after_moving() {
        let mut dedup = FrameDeduplicator::new(DEDUP_THRESHOLD, 3);

        assert_eq!(dedup.push_frame(make_frame(0, TFS)).len(), 1);
        for i in 0..3 {
            let out = dedup.push_frame(make_frame(0, TFS));
            assert_eq!(out.len(), 1, "look-ahead frame {} should output", i);
        }
        assert_eq!(dedup.push_frame(make_frame(0, TFS)).len(), 0);
    }

    #[test]
    fn test_dedup_moving_resets_countdown() {
        let mut dedup = FrameDeduplicator::new(DEDUP_THRESHOLD, 3);

        dedup.push_frame(make_frame(0, TFS));
        dedup.push_frame(make_frame(0, TFS));
        let out = dedup.push_frame(make_frame(100, TFS));
        assert_eq!(out.len(), 1);

        for _ in 0..3 {
            assert_eq!(dedup.push_frame(make_frame(100, TFS)).len(), 1);
        }
        assert_eq!(dedup.push_frame(make_frame(100, TFS)).len(), 0);
    }

    #[test]
    fn test_dedup_flush_discards_buffer() {
        let mut dedup = FrameDeduplicator::new(DEDUP_THRESHOLD, 3);
        let frame = make_frame(128, TFS);

        dedup.push_frame(frame.clone());
        for _ in 0..3 {
            dedup.push_frame(frame.clone());
        }
        for _ in 0..5 {
            dedup.push_frame(frame.clone());
        }

        assert!(dedup.flush().is_empty());
    }

    #[test]
    fn test_dedup_single_frame() {
        let mut dedup = FrameDeduplicator::new(DEDUP_THRESHOLD, 3);
        assert_eq!(dedup.push_frame(make_frame(42, TFS)).len(), 1);
        assert!(dedup.flush().is_empty());
    }

    #[test]
    fn test_dedup_empty_flush() {
        let mut dedup = FrameDeduplicator::new(DEDUP_THRESHOLD, 3);
        assert!(dedup.flush().is_empty());
    }

    // --- Marker frame ---

    #[test]
    fn test_marker_frame_correct_size() {
        let frame = generate_marker_frame(1920, 1080, "Title", "Description");
        assert_eq!(frame.len(), 1920 * 1080 * 3);
    }

    #[test]
    fn test_marker_frame_small_dimensions() {
        let frame = generate_marker_frame(TW, TH, "Hi", "World");
        assert_eq!(frame.len(), TFS);
    }

    // --- Pipeline integration tests with mock ffmpeg ---

    #[tokio::test]
    async fn test_pipeline_dedup_with_mock_ffmpeg() {
        let look_window = 2;
        let (_msg_tx, msg_rx) = mpsc::channel::<RecordingMessage>(16);

        let (mut capture_write, capture_read) = duplex(TFS * 8);
        let (encoder_write, mut encoder_read) = duplex(TFS * 32);

        let pipeline = tokio::spawn(async move {
            run_pipeline(
                capture_read,
                encoder_write,
                msg_rx,
                TW,
                TH,
                DEDUP_THRESHOLD,
                look_window,
                0,
            )
            .await
        });

        // F0(128): first → moving
        // F1(128): still, countdown=2→1
        // F2(128): still, countdown=1→0
        // F3(128): still, buffered
        // F4(  0): moving → flush buffer(F3) + F4, countdown=2
        // F5(  0): still, countdown=2→1
        for _ in 0..4 {
            capture_write.write_all(&make_frame(128, TFS)).await.unwrap();
        }
        for _ in 0..2 {
            capture_write.write_all(&make_frame(0, TFS)).await.unwrap();
        }

        drop(capture_write);
        pipeline.await.unwrap().unwrap();

        let mut output = Vec::new();
        encoder_read.read_to_end(&mut output).await.unwrap();
        assert_eq!(output.len() / TFS, 6);
    }

    #[tokio::test]
    async fn test_pipeline_discards_long_still_sequence() {
        let look_window = 2;
        let (_msg_tx, msg_rx) = mpsc::channel::<RecordingMessage>(16);

        let (mut capture_write, capture_read) = duplex(TFS * 32);
        let (encoder_write, mut encoder_read) = duplex(TFS * 32);

        let pipeline = tokio::spawn(async move {
            run_pipeline(
                capture_read,
                encoder_write,
                msg_rx,
                TW,
                TH,
                DEDUP_THRESHOLD,
                look_window,
                0,
            )
            .await
        });

        capture_write.write_all(&make_frame(128, TFS)).await.unwrap();
        for _ in 0..20 {
            capture_write.write_all(&make_frame(128, TFS)).await.unwrap();
        }
        drop(capture_write);

        pipeline.await.unwrap().unwrap();

        let mut output = Vec::new();
        encoder_read.read_to_end(&mut output).await.unwrap();
        assert_eq!(output.len() / TFS, 3);
    }

    #[tokio::test]
    async fn test_pipeline_marker_injection() {
        let marker_count = 5;
        let (msg_tx, msg_rx) = mpsc::channel(16);

        let (mut capture_write, capture_read) = duplex(TFS * 8);
        let (encoder_write, mut encoder_read) = duplex(TFS * 64);

        let pipeline = tokio::spawn(async move {
            run_pipeline(
                capture_read,
                encoder_write,
                msg_rx,
                TW,
                TH,
                DEDUP_THRESHOLD,
                2,
                marker_count,
            )
            .await
        });

        capture_write.write_all(&make_frame(128, TFS)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        msg_tx
            .send(RecordingMessage::MarkerFrame {
                title: "Test".into(),
                description: "Desc".into(),
            })
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        drop(capture_write);
        drop(msg_tx);

        pipeline.await.unwrap().unwrap();

        let mut output = Vec::new();
        encoder_read.read_to_end(&mut output).await.unwrap();

        let count = output.len() / TFS;
        // At least the marker frames (5) + the first frame (1)
        assert!(
            count >= marker_count + 1,
            "expected at least {} frames, got {}",
            marker_count + 1,
            count
        );
    }

    #[tokio::test]
    async fn test_pipeline_empty_recording() {
        let (_msg_tx, msg_rx) = mpsc::channel(16);

        let (capture_write, capture_read) = duplex(TFS * 4);
        let (encoder_write, mut encoder_read) = duplex(TFS * 4);

        drop(capture_write);

        let result = run_pipeline(
            capture_read,
            encoder_write,
            msg_rx,
            TW,
            TH,
            DEDUP_THRESHOLD,
            2,
            0,
        )
        .await;
        assert!(result.is_ok());

        let mut output = Vec::new();
        encoder_read.read_to_end(&mut output).await.unwrap();
        assert_eq!(output.len(), 0);
    }

    #[tokio::test]
    async fn test_pipeline_alternating_frames() {
        let look_window = 1;
        let (_msg_tx, msg_rx) = mpsc::channel::<RecordingMessage>(16);

        let (mut capture_write, capture_read) = duplex(TFS * 32);
        let (encoder_write, mut encoder_read) = duplex(TFS * 32);

        let pipeline = tokio::spawn(async move {
            run_pipeline(
                capture_read,
                encoder_write,
                msg_rx,
                TW,
                TH,
                DEDUP_THRESHOLD,
                look_window,
                0,
            )
            .await
        });

        for i in 0..10u8 {
            let val = if i % 2 == 0 { 0 } else { 255 };
            capture_write.write_all(&make_frame(val, TFS)).await.unwrap();
        }

        drop(capture_write);
        pipeline.await.unwrap().unwrap();

        let mut output = Vec::new();
        encoder_read.read_to_end(&mut output).await.unwrap();
        assert_eq!(output.len() / TFS, 10);
    }
}
