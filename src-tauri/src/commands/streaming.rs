//! Tauri command surface for the RAOP multi-sender (Phase S2).
//!
//! Thin wrappers over [`StreamEngine`] (held in `AppState`). Errors are mapped to
//! [`AppError::Device`] (a receiver/child problem the UI shows inline) or
//! [`AppError::Internal`] (binary missing / bug). The engine also emits
//! `stream-state` events on every transition; these commands return the same
//! [`StreamStatus`] synchronously for the caller that triggered the change.

use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::streaming::model::{StreamSource, StreamStatus, StreamTarget};

/// Start streaming `source` to every target in sync.
#[tauri::command]
pub async fn stream_start(
    app: AppHandle,
    targets: Vec<StreamTarget>,
    source: StreamSource,
) -> AppResult<StreamStatus> {
    let engine = &app.state::<AppState>().streaming;
    let bin = engine.resolve_binary().ok_or_else(|| {
        AppError::Internal(
            "cliraop sender binary not found (run scripts/fetch_cliraop.sh)".into(),
        )
    })?;
    engine
        .start(bin, targets, source, None, None)
        .map_err(AppError::Device)
}

/// Stop the active stream (kills every child). Idempotent.
#[tauri::command]
pub async fn stream_stop(app: AppHandle) -> AppResult<StreamStatus> {
    app.state::<AppState>()
        .streaming
        .stop()
        .map_err(AppError::Device)
}

/// Set one receiver's software volume live (`0.0..=1.0`).
#[tauri::command]
pub async fn stream_set_device_volume(
    app: AppHandle,
    ip: String,
    vol: f32,
) -> AppResult<StreamStatus> {
    app.state::<AppState>()
        .streaming
        .set_device_volume(&ip, vol)
        .map_err(AppError::Device)
}

/// Set one receiver's software delay in milliseconds.
#[tauri::command]
pub async fn stream_set_device_delay(
    app: AppHandle,
    ip: String,
    ms: u32,
) -> AppResult<StreamStatus> {
    app.state::<AppState>()
        .streaming
        .set_device_delay(&ip, ms)
        .map_err(AppError::Device)
}

/// Current streaming status (idle when nothing is playing).
#[tauri::command]
pub async fn stream_status(app: AppHandle) -> AppResult<StreamStatus> {
    Ok(app.state::<AppState>().streaming.status())
}
