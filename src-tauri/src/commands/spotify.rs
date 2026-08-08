//! Tauri command surface for the Spotify Connect capture (Phase S3).
//!
//! Thin wrappers over [`SpotifyManager`](crate::spotify::SpotifyManager) (held in
//! `AppState`). Lifecycle/transport errors map to [`AppError::Device`] (shown
//! inline in the UI). The manager also emits `spotify-state` events on every
//! transition; these commands return the current [`SpotifyState`] synchronously.

use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};
use crate::spotify::SpotifyState;
use crate::state::AppState;

/// Start advertising the "MusicSync" Spotify Connect endpoint. The user then
/// selects it in their Spotify app (Premium) to begin streaming.
#[tauri::command]
pub async fn spotify_start(app: AppHandle) -> AppResult<SpotifyState> {
    app.state::<AppState>()
        .spotify
        .start()
        .map_err(AppError::Device)
}

/// Stop advertising and tear down the capture session. Idempotent.
#[tauri::command]
pub async fn spotify_stop(app: AppHandle) -> AppResult<SpotifyState> {
    app.state::<AppState>()
        .spotify
        .stop()
        .map_err(AppError::Device)
}

/// Current capture state (running / connected / now-playing).
#[tauri::command]
pub async fn spotify_status(app: AppHandle) -> AppResult<SpotifyState> {
    Ok(app.state::<AppState>().spotify.status())
}

/// Resume playback on the connected Spotify session.
#[tauri::command]
pub async fn spotify_play(app: AppHandle) -> AppResult<()> {
    app.state::<AppState>()
        .spotify
        .play()
        .map_err(AppError::Device)
}

/// Pause playback.
#[tauri::command]
pub async fn spotify_pause(app: AppHandle) -> AppResult<()> {
    app.state::<AppState>()
        .spotify
        .pause()
        .map_err(AppError::Device)
}

/// Skip to the next track.
#[tauri::command]
pub async fn spotify_next(app: AppHandle) -> AppResult<()> {
    app.state::<AppState>()
        .spotify
        .next()
        .map_err(AppError::Device)
}

/// Skip to the previous track.
#[tauri::command]
pub async fn spotify_prev(app: AppHandle) -> AppResult<()> {
    app.state::<AppState>()
        .spotify
        .prev()
        .map_err(AppError::Device)
}

/// Set Connect volume from a `0.0..=1.0` level.
#[tauri::command]
pub async fn spotify_set_volume(app: AppHandle, level: f32) -> AppResult<()> {
    app.state::<AppState>()
        .spotify
        .set_volume(level)
        .map_err(AppError::Device)
}
