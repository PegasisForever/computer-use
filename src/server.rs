use crate::config::*;
use crate::{keyboard, mouse, recording, screenshot};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep};

// ---------------------------------------------------------------------------
// Tool parameter types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClickParams {
    #[schemars(description = "X coordinate in 1456x819 space")]
    pub x: Option<f64>,
    #[schemars(description = "Y coordinate in 1456x819 space")]
    pub y: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MouseMoveParams {
    #[schemars(description = "X coordinate in 1456x819 space")]
    pub x: f64,
    #[schemars(description = "Y coordinate in 1456x819 space")]
    pub y: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrollParams {
    #[schemars(description = "X coordinate in 1456x819 space")]
    pub x: Option<f64>,
    #[schemars(description = "Y coordinate in 1456x819 space")]
    pub y: Option<f64>,
    #[schemars(description = "Scroll amount in notches (one unit = one scroll wheel notch). Positive scrolls down, negative scrolls up, must be between -5 and 5")]
    pub amount: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyParams {
    #[schemars(description = "Key or key combination (e.g. ctrl+c, Return, alt+Tab)")]
    pub keys: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeParams {
    #[schemars(description = "Text string to type on the keyboard")]
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkerParams {
    #[schemars(description = "Marker title text")]
    pub title: String,
    #[schemars(description = "Marker description text")]
    pub description: String,
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

fn mcp_err(e: anyhow::Error) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

fn screenshot_content(base64_data: String) -> Content {
    Content::image(base64_data, "image/png")
}

async fn take_screenshot_content() -> Result<Content, ErrorData> {
    let data = screenshot::capture_screenshot().await.map_err(mcp_err)?;
    Ok(screenshot_content(data))
}

#[derive(Clone)]
pub struct ComputerUseServer {
    tool_router: ToolRouter<Self>,
    recording: Arc<Mutex<Option<recording::RecordingHandle>>>,
}

#[tool_router]
impl ComputerUseServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            recording: Arc::new(Mutex::new(None)),
        }
    }

    // --- Mouse & Keyboard Actions ---

    #[tool(description = "Left click. Multi-screenshot flow: press down, capture pressed state, release, capture loading state, wait, capture loaded state. Returns 3 screenshots. Optionally move to (x, y) first.")]
    async fn left_click(
        &self,
        Parameters(params): Parameters<ClickParams>,
    ) -> Result<CallToolResult, ErrorData> {
        mouse::maybe_move(params.x, params.y)
            .await
            .map_err(mcp_err)?;

        mouse::left_mouse_down().await.map_err(mcp_err)?;

        sleep(Duration::from_secs_f64(LEFT_CLICK_PRESS_WAIT_SECS)).await;
        let ss1 = take_screenshot_content().await?;

        mouse::left_mouse_up().await.map_err(mcp_err)?;
        sleep(Duration::from_secs_f64(LEFT_CLICK_RELEASE_WAIT_SECS)).await;
        let ss2 = take_screenshot_content().await?;

        sleep(Duration::from_secs_f64(ACTION_WAIT_SECS)).await;
        let ss3 = take_screenshot_content().await?;

        Ok(CallToolResult::success(vec![ss1, ss2, ss3]))
    }

    #[tool(description = "Double-click the left mouse button. Optionally move to (x, y) first. Returns a screenshot.")]
    async fn left_double_click(
        &self,
        Parameters(params): Parameters<ClickParams>,
    ) -> Result<CallToolResult, ErrorData> {
        mouse::maybe_move(params.x, params.y)
            .await
            .map_err(mcp_err)?;
        mouse::left_double_click().await.map_err(mcp_err)?;
        sleep(Duration::from_secs_f64(ACTION_WAIT_SECS)).await;
        let ss = take_screenshot_content().await?;
        Ok(CallToolResult::success(vec![ss]))
    }

    #[tool(description = "Click the right mouse button. Optionally move to (x, y) first. Returns a screenshot.")]
    async fn right_click(
        &self,
        Parameters(params): Parameters<ClickParams>,
    ) -> Result<CallToolResult, ErrorData> {
        mouse::maybe_move(params.x, params.y)
            .await
            .map_err(mcp_err)?;
        mouse::right_click().await.map_err(mcp_err)?;
        sleep(Duration::from_secs_f64(ACTION_WAIT_SECS)).await;
        let ss = take_screenshot_content().await?;
        Ok(CallToolResult::success(vec![ss]))
    }

    #[tool(description = "Click the middle mouse button. Optionally move to (x, y) first. Returns a screenshot.")]
    async fn middle_click(
        &self,
        Parameters(params): Parameters<ClickParams>,
    ) -> Result<CallToolResult, ErrorData> {
        mouse::maybe_move(params.x, params.y)
            .await
            .map_err(mcp_err)?;
        mouse::middle_click().await.map_err(mcp_err)?;
        sleep(Duration::from_secs_f64(ACTION_WAIT_SECS)).await;
        let ss = take_screenshot_content().await?;
        Ok(CallToolResult::success(vec![ss]))
    }

    #[tool(description = "Smoothly move the mouse to (x, y) using ease-in-out interpolation. Returns a screenshot.")]
    async fn mouse_move(
        &self,
        Parameters(params): Parameters<MouseMoveParams>,
    ) -> Result<CallToolResult, ErrorData> {
        mouse::move_to_scaled(params.x, params.y)
            .await
            .map_err(mcp_err)?;
        sleep(Duration::from_secs_f64(ACTION_WAIT_SECS)).await;
        let ss = take_screenshot_content().await?;
        Ok(CallToolResult::success(vec![ss]))
    }

    #[tool(description = "Scroll at the current or specified position. Positive amount scrolls down, negative scrolls up. Optionally move to (x, y) first. Returns a screenshot.")]
    async fn scroll(
        &self,
        Parameters(params): Parameters<ScrollParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if params.amount < -5 || params.amount > 5 {
            return Err(ErrorData::invalid_params(
                "scroll amount must be between -5 and 5",
                None,
            ));
        }
        mouse::maybe_move(params.x, params.y)
            .await
            .map_err(mcp_err)?;
        mouse::scroll(params.amount).await.map_err(mcp_err)?;
        sleep(Duration::from_secs_f64(ACTION_WAIT_SECS)).await;
        let ss = take_screenshot_content().await?;
        Ok(CallToolResult::success(vec![ss]))
    }

    #[tool(description = "Take a screenshot and return it. No action performed.")]
    async fn screenshot(&self) -> Result<CallToolResult, ErrorData> {
        let ss = take_screenshot_content().await?;
        Ok(CallToolResult::success(vec![ss]))
    }

    #[tool(description = "Press a key or key combination (e.g. ctrl+c, Return, alt+Tab). Returns a screenshot.")]
    async fn key(
        &self,
        Parameters(params): Parameters<KeyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        keyboard::key_press(&params.keys)
            .await
            .map_err(mcp_err)?;
        sleep(Duration::from_secs_f64(ACTION_WAIT_SECS)).await;
        let ss = take_screenshot_content().await?;
        Ok(CallToolResult::success(vec![ss]))
    }

    #[tool(name = "type", description = "Type a string of text on the keyboard. Returns a screenshot.")]
    async fn type_text(
        &self,
        Parameters(params): Parameters<TypeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        keyboard::type_text(&params.text)
            .await
            .map_err(mcp_err)?;
        sleep(Duration::from_secs_f64(ACTION_WAIT_SECS)).await;
        let ss = take_screenshot_content().await?;
        Ok(CallToolResult::success(vec![ss]))
    }

    // --- Screen Recording ---

    #[tool(description = "Begin recording the screen. The recording is saved to a file in /tmp/.")]
    async fn start_recording(&self) -> Result<CallToolResult, ErrorData> {
        let mut guard = self.recording.lock().await;
        if guard.is_some() {
            return Err(ErrorData::invalid_request(
                "Recording already in progress",
                None,
            ));
        }
        let handle = recording::start_recording().map_err(mcp_err)?;
        let msg = format!(
            "Recording started. Output will be saved to: {}",
            handle.output_path
        );
        *guard = Some(handle);
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    #[tool(description = "Stop the current recording and return the output file path.")]
    async fn stop_recording(&self) -> Result<CallToolResult, ErrorData> {
        let mut guard = self.recording.lock().await;
        let handle = guard.take().ok_or_else(|| {
            ErrorData::invalid_request("No recording in progress", None)
        })?;
        drop(guard);

        let path = handle.stop().await.map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Recording saved to: {}",
            path
        ))]))
    }

    #[tool(description = "Insert a marker scene into the current recording: a white frame with title and description text, displayed for 3 seconds.")]
    async fn add_recording_marker(
        &self,
        Parameters(params): Parameters<MarkerParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let guard = self.recording.lock().await;
        let handle = guard.as_ref().ok_or_else(|| {
            ErrorData::invalid_request("No recording in progress", None)
        })?;
        handle
            .add_marker(params.title, params.description)
            .await
            .map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            "Marker added to recording",
        )]))
    }
}

#[tool_handler]
impl ServerHandler for ComputerUseServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Computer use MCP server providing mouse, keyboard, screenshot, and screen recording capabilities on X11.",
            )
    }
}
