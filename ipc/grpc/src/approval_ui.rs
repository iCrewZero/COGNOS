//! HAL UI socket wire helpers — length-prefixed JSON on `/run/cognos/hal-ui.sock`.
//!
//! HAL connects as a **client**; `cognos approval watch` listens as the **server**.
//! Response may include an optional `notice` field (ignored by HAL v0; surfaced in CLI UX).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_HAL_UI_SOCKET: &str = "/run/cognos/hal-ui.sock";
pub const MAX_UI_FRAME_BYTES: usize = 4096;

/// Dialog pushed by HAL when a gate request needs human confirmation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiDialogRequest {
    pub r#type: String,
    pub request_id: Uuid,
    pub action: String,
    pub target: String,
    pub agent: String,
    pub hal_score: f32,
    pub is_ai_generated: bool,
}

impl UiDialogRequest {
    pub fn short_id(&self) -> String {
        self.request_id.to_string()[..8].to_string()
    }

    pub fn level_label(&self) -> &'static str {
        if self.r#type == "block_dialog" {
            "BLOCK"
        } else {
            "CONFIRM"
        }
    }
}

/// Human decision returned to HAL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiDialogResponse {
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

pub fn read_frame(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_UI_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds max {MAX_UI_FRAME_BYTES}"),
        ));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

pub fn write_frame(stream: &mut UnixStream, payload: &[u8]) -> std::io::Result<()> {
    if payload.len() > MAX_UI_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "payload too large for UI frame",
        ));
    }
    let len = (payload.len() as u32).to_be_bytes();
    stream.write_all(&len)?;
    stream.write_all(payload)?;
    stream.flush()
}

pub fn read_dialog(stream: &mut UnixStream) -> std::io::Result<UiDialogRequest> {
    let payload = read_frame(stream)?;
    serde_json::from_slice(&payload).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid UI dialog JSON: {e}"),
        )
    })
}

pub fn write_dialog_response(
    stream: &mut UnixStream,
    response: &UiDialogResponse,
) -> std::io::Result<()> {
    let payload = serde_json::to_vec(response).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("serialize UI response: {e}"),
        )
    })?;
    write_frame(stream, &payload)
}

pub fn socket_parent_ready(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}
