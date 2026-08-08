//! Settings commands (brief.md FR-20 / FR-21 / FR-27).
//!
//! The settings surface: the sweep subnet, the poll-interval floor, the request
//! timeout, the UI theme, launch-at-login, plus config export/import (FR-21).
//!
//! Note: grouping settings (Group Guard, master failover) are intentionally
//! absent — the LP10's native DDMS grouping is non-functional
//! (docs/firmware-notes.md §G/§H), so there is nothing for them to control. The
//! two fields survive on-disk for back-compat but are never surfaced here.

use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;

use crate::discovery;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::{self, ConfigBundle, Settings, THEMES};

/// The persisted preferences, as the settings page renders them.
#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    store::get(&app).settings
}

/// A partial settings update (FR-20 / FR-27): only the fields the user actually
/// changed are `Some`. Field names mirror [`Settings`] so the JSON stays 1:1.
/// `subnet` keeps its own command ([`set_subnet`]) because it needs CIDR parsing.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SettingsPatch {
    pub poll_ms: Option<u64>,
    pub theme: Option<String>,
    pub http_timeout_ms: Option<u64>,
    pub start_at_login: Option<bool>,
}

/// Apply a partial update. Values are validated then clamped by
/// [`Settings::sanitize`], so an out-of-range poll interval can't be persisted.
#[tauri::command]
pub fn update_settings(app: AppHandle, patch: SettingsPatch) -> AppResult<Settings> {
    if let Some(theme) = patch.theme.as_deref() {
        if !THEMES.contains(&theme) {
            return Err(AppError::InvalidInput(format!("Unknown theme “{theme}”.")));
        }
    }

    // start_at_login is more than a stored flag — it registers a launch agent
    // with the OS. Toggle first, then persist whatever state actually took, so
    // a failed OS call is never remembered as success.
    let effective_autostart = patch.start_at_login.map(|want| apply_autostart(&app, want));

    store::update(&app, |config| {
        let s = &mut config.settings;
        if let Some(v) = patch.poll_ms {
            s.poll_ms = v;
        }
        if let Some(v) = patch.http_timeout_ms {
            s.http_timeout_ms = v;
        }
        if let Some(v) = patch.theme {
            s.theme = v;
        }
        if let Some(v) = effective_autostart {
            s.start_at_login = v;
        }
        s.sanitize();
        Ok(s.clone())
    })
}

/// Toggle the OS launch-at-login registration and report the resulting state.
/// A failure is logged, not fatal: the checkbox reflects reality on the next
/// read rather than lying about a toggle that didn't take.
fn apply_autostart(app: &AppHandle, enable: bool) -> bool {
    let mgr = app.autolaunch();
    let outcome = if enable { mgr.enable() } else { mgr.disable() };
    if let Err(e) = outcome {
        log::error!("autostart {} failed: {e}", if enable { "enable" } else { "disable" });
    }
    mgr.is_enabled().unwrap_or(enable)
}

/// FR-20: override the CIDR the discovery sweep defaults to.
///
/// Validated with the same parser the scan uses (including the /16 cap), so an
/// unsweepable range is rejected here rather than at the start of every scan.
/// `None` — or a blank string — restores auto-detection from the local address.
#[tauri::command]
pub fn set_subnet(app: AppHandle, cidr: Option<String>) -> AppResult<Settings> {
    let cidr = match cidr.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(raw) => Some(discovery::parse_cidr(raw)?.to_string()),
        None => None,
    };
    store::update(&app, |config| {
        config.settings.subnet = cidr;
        Ok(config.settings.clone())
    })
}

/// Window focus/blur → adaptive poll cadence (2 s focused, 5 s blurred). Wired
/// from `+layout.svelte`'s focus/blur listeners. Cheap and idempotent, so the
/// frontend can fire it freely.
#[tauri::command]
pub fn set_poll_profile(app: AppHandle, focused: bool) {
    app.state::<AppState>().poller.set_focused(focused);
}

/// FR-21: serialise settings + saved devices to JSON, with the auth hash (and
/// the "remember me" grant) stripped by construction. The UI writes the string
/// to a file the user picks via the dialog plugin.
#[tauri::command]
pub fn export_config(app: AppHandle) -> AppResult<String> {
    let config = store::get(&app);
    let bundle = store::export_bundle(&config);
    serde_json::to_string_pretty(&bundle).map_err(AppError::from)
}

/// FR-21: merge an exported bundle back in. Settings are replaced (and
/// sanitised), devices are unioned by uuid; auth is never touched. Newly
/// imported devices start polling at once, so they light up without a restart.
#[tauri::command]
pub fn import_config(app: AppHandle, json: String) -> AppResult<Settings> {
    let bundle: ConfigBundle = serde_json::from_str(&json)
        .map_err(|e| AppError::InvalidInput(format!("That file isn't a MusicSync config: {e}")))?;

    let settings = store::update(&app, |config| {
        store::merge_bundle(config, bundle);
        Ok(config.settings.clone())
    })?;

    // Re-poll the (possibly enlarged) device list. `start` is idempotent.
    crate::poller::start_saved(&app);
    Ok(settings)
}

const CONFIG_FILE_NAME: &str = "musicsync-config.json";
const CONFIG_FILTER: (&str, &[&str]) = ("MusicSync config", &["json"]);

/// FR-21 (UI path): pick a destination with the save dialog, then write the
/// export there. Returns `false` when the user cancels the picker. The dialog
/// callback is bridged to async over a oneshot so the command stays off the
/// UI thread.
#[tauri::command]
pub async fn export_config_file(app: AppHandle) -> AppResult<bool> {
    let json = export_config(app.clone())?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter(CONFIG_FILTER.0, CONFIG_FILTER.1)
        .set_file_name(CONFIG_FILE_NAME)
        .save_file(move |path| {
            let _ = tx.send(path);
        });

    let Some(path) = rx.await.map_err(|e| AppError::Internal(e.to_string()))? else {
        return Ok(false); // cancelled
    };
    let path = path.into_path().map_err(|e| AppError::Internal(e.to_string()))?;
    std::fs::write(&path, json).map_err(|e| AppError::Store(format!("could not write {}: {e}", path.display())))?;
    Ok(true)
}

/// FR-21 (UI path): pick a config file with the open dialog, read it, and merge.
/// Returns `None` when the user cancels; otherwise the merged settings.
#[tauri::command]
pub async fn import_config_file(app: AppHandle) -> AppResult<Option<Settings>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter(CONFIG_FILTER.0, CONFIG_FILTER.1)
        .pick_file(move |path| {
            let _ = tx.send(path);
        });

    let Some(path) = rx.await.map_err(|e| AppError::Internal(e.to_string()))? else {
        return Ok(None); // cancelled
    };
    let path = path.into_path().map_err(|e| AppError::Internal(e.to_string()))?;
    let json = std::fs::read_to_string(&path)
        .map_err(|e| AppError::InvalidInput(format!("could not read {}: {e}", path.display())))?;

    let settings = import_config(app, json)?;
    Ok(Some(settings))
}
