//! Discovery commands (brief.md FR-4).
//!
//! `scan` is long-running by nature: it resolves with the full candidate list,
//! but the UI does not wait for that — `scan-progress` / `scan-device-found`
//! stream while it runs, so results appear as they are confirmed.
//!
//! Candidates are **not** persisted here. Adding stays FR-5's `add_device`,
//! which re-validates the address, writes the config and starts a poll task —
//! one path into the device list, not two.

use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::discovery::{self, DeviceCandidate, ScanOptions, ScanToken};
use crate::error::AppResult;
use crate::state::AppState;

/// FR-4: mDNS + SSDP + (optionally) a subnet sweep, run concurrently.
///
/// Only one scan runs at a time. A second request **cancels the first and
/// restarts** rather than erroring: the user pressing "Scan" again means "do it
/// now", and a refusal would read as a dead button. The superseded scan emits
/// its own `scan-complete { cancelled: true }`, which the store ignores because
/// it is no longer the current run.
#[tauri::command]
pub async fn scan(app: AppHandle, options: Option<ScanOptions>) -> AppResult<Vec<DeviceCandidate>> {
    // Scoped so the `State` guard is dropped before the first await point.
    let token: Arc<ScanToken> = app.state::<AppState>().scan.begin();

    let result = discovery::run(app.clone(), options.unwrap_or_default(), token.clone()).await;

    // Release the slot even on the error path, or a rejected CIDR would leave
    // the app believing a scan is still running.
    app.state::<AppState>().scan.finish(&token);
    result
}

/// Stop the running scan. False when there was nothing to stop.
///
/// Returns immediately: the strategies observe the token at their next await
/// point and the command's own future resolves with whatever was already found.
#[tauri::command]
pub fn cancel_scan(app: AppHandle) -> bool {
    app.state::<AppState>().scan.cancel()
}
