//! Tauri command surface for the RAOP multi-sender (Phase S2).
//!
//! Thin wrappers over [`StreamEngine`] (held in `AppState`). Errors are mapped to
//! [`AppError::Device`] (a receiver/child problem the UI shows inline) or
//! [`AppError::Internal`] (binary missing / bug). The engine also emits
//! `stream-state` events on every transition; these commands return the same
//! [`StreamStatus`] synchronously for the caller that triggered the change.

use std::collections::BTreeMap;

use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::{self, ManualTarget};
use crate::streaming::model::{StreamSource, StreamStatus, StreamTarget};

/// Start streaming `source` to every target in sync.
///
/// The `Spotify` source is live (Phase S3): its PCM is teed from the running
/// Spotify capture's fan-out via the engine's `start_live` path rather than loaded
/// from a file. Every other source is the S2 static path.
#[tauri::command]
pub async fn stream_start(
    app: AppHandle,
    mut targets: Vec<StreamTarget>,
    source: StreamSource,
) -> AppResult<StreamStatus> {
    // Feature 2: seed each target's initial delay from the persisted store so a
    // pre-tuned offset is applied from the very first frame.
    let config = store::get(&app);
    for target in targets.iter_mut() {
        target.delay_ms = config.target_delay(&target.delay_key());
    }

    let state = app.state::<AppState>();
    let engine = &state.streaming;
    let bin = engine.resolve_binary().ok_or_else(|| {
        AppError::Internal(
            "cliraop sender binary not found (run scripts/fetch_cliraop.sh)".into(),
        )
    })?;
    match source {
        StreamSource::Spotify => {
            if !state.spotify.is_running() {
                return Err(AppError::Device(
                    "start Spotify capture first (spotify_start), then pick MusicSync in Spotify"
                        .into(),
                ));
            }
            let fanout = state.spotify.fanout();
            engine
                .start_live(bin, targets, fanout, None, None)
                .map_err(AppError::Device)
        }
        other => engine
            .start(bin, targets, other, None, None)
            .map_err(AppError::Device),
    }
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

// --------------------------------------------------------- manual targets (F1) --

/// Add a manual RAOP receiver (name + `ip:port`) so the "Play Everywhere" flow is
/// testable without real LP10s — e.g. a local `shairport-sync` instance. Returns
/// the full manual-target list so the picker refreshes in one round trip.
#[tauri::command]
pub async fn add_manual_target(
    app: AppHandle,
    name: String,
    ip: String,
    port: u16,
) -> AppResult<Vec<ManualTarget>> {
    store::update(&app, |config| {
        config.add_manual_target(&name, &ip, port)?;
        Ok(config.manual_targets.clone())
    })
}

/// Remove a manual target by id (also forgets any delay saved under it). Returns
/// the remaining list. Idempotent — removing an unknown id is a no-op.
#[tauri::command]
pub async fn remove_manual_target(app: AppHandle, id: String) -> AppResult<Vec<ManualTarget>> {
    store::update(&app, |config| {
        config.remove_manual_target(&id);
        Ok(config.manual_targets.clone())
    })
}

/// The persisted manual targets.
#[tauri::command]
pub async fn list_manual_targets(app: AppHandle) -> Vec<ManualTarget> {
    store::get(&app).manual_targets
}

// ------------------------------------------------------- per-device delay (F2) --

/// The persisted per-target delays, keyed by device UUID / manual-id / IP.
#[tauri::command]
pub async fn list_target_delays(app: AppHandle) -> BTreeMap<String, u32> {
    store::get(&app).device_delays
}

/// Persist a per-target playback delay (clamped to `0..=2000` ms) and, when a
/// stream is live, apply it to the matching receiver. Works whether or not a
/// stream is running, so the user can pre-tune offsets. Returns the clamped ms.
#[tauri::command]
pub async fn set_target_delay(app: AppHandle, key: String, ms: u32) -> AppResult<u32> {
    let ms = store::clamp_delay(ms);
    store::update(&app, |config| {
        config.set_target_delay(&key, ms);
        Ok(())
    })?;
    let state = app.state::<AppState>();
    if state.streaming.is_active() {
        // Best effort: the key may not be in the live group (pre-tuning a speaker
        // that isn't currently streaming), which is not an error here.
        let _ = state.streaming.set_target_delay(&key, ms);
    }
    Ok(ms)
}
