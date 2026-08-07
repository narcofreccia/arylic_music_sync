//! Settings commands (brief.md FR-20).
//!
//! M3 needs exactly one of these: the sweep's default subnet. The rest of FR-20
//! / FR-27 (polling interval, theme, Group Guard, failover) lands with M6, which
//! will extend this module rather than start a new one.

use tauri::AppHandle;

use crate::discovery;
use crate::error::AppResult;
use crate::store::{self, Settings};

/// The persisted preferences, as the settings page renders them.
#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    store::get(&app).settings
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
