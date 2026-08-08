//! Persisted configuration — a single `settings.json` in the app config dir,
//! managed by tauri-plugin-store.
//!
//! The whole file is mirrored in memory (`AppState::config`) so reads are free;
//! every mutation goes through [`update`], which writes the mirror and flushes
//! to disk in one step. Top-level `Config` fields map 1:1 to store keys, so the
//! on-disk JSON stays readable/hand-editable.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_store::StoreExt;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub const STORE_FILE: &str = "settings.json";

/// Store keys we own. Anything else in the file is left untouched.
const KEYS: [&str; 4] = ["auth", "devices", "settings", "remember_me"];

/// The local profile (FR-1). No cloud, no account server: just a username and
/// an Argon2 PHC string.
///
/// `password_hash` is optional because FR-3 lets the user *remove* the password
/// without deleting the profile — the profile itself is what tells first launch
/// apart from "configured, but opens without a prompt".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub username: String,
    #[serde(default)]
    pub password_hash: Option<String>,
}

/// A device the user added or discovered (FR-5/FR-6).
///
/// The UUID is the identity — it survives a DHCP lease change, which the IP
/// does not, so it keys the config, the poller and every event. Live state
/// (online, role, playback) is deliberately *not* stored here: it belongs to
/// the poller's in-memory cache and would only ever be stale on disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
// Field-level defaults across the board: a config written by an older build
// (or hand-edited) must degrade, never fail the whole load.
#[serde(default)]
pub struct SavedDevice {
    /// UPnP UDN uuid — the stable identity across DHCP leases.
    pub uuid: String,
    /// DDMS `USN` (a MAC), a fallback identity when no UPnP uuid was seen.
    pub usn: String,
    pub ip: String,
    /// Local friendly name (FR-7); `None` = show the device's own name.
    pub alias: Option<String>,
    /// Last known transport (`ethernet` | `wifi`), for the offline card badge.
    pub net_mode: Option<String>,
    /// Unix ms of the last successful contact, so an offline device can still
    /// say when it was last around.
    pub last_seen: Option<i64>,
    /// Added by hand (FR-5) rather than discovered — the scan must not prune
    /// these just because they didn't answer a sweep.
    pub pinned_manual: bool,
}

/// User preferences (brief.md FR-20 / FR-27). Present with defaults from M1 so
/// later milestones only have to read them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Status poll interval in ms (NFR-7: guard detection ≤ 1 cycle).
    pub poll_ms: u64,
    /// CIDR for the subnet sweep; `None` = auto-detect from local interfaces.
    pub subnet: Option<String>,
    pub theme: String,
    /// Group Guard behaviour: "ask" | "always" | "never".
    pub guard_mode: String,
    /// Master-offline failover: "prompt" | "auto" | "never".
    pub failover_mode: String,
    pub start_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_ms: 3000,
            subnet: None,
            theme: "dark".into(),
            guard_mode: "ask".into(),
            failover_mode: "prompt".into(),
            start_at_login: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub auth: Option<AuthConfig>,
    pub devices: Vec<SavedDevice>,
    pub settings: Settings,
    /// FR-2: skip the login screen on this machine.
    pub remember_me: bool,
}

/// Read `settings.json` once at startup. A missing or partially-written file is
/// not fatal — missing keys fall back to defaults (first launch is exactly the
/// "everything missing" case).
pub fn load<R: Runtime>(app: &AppHandle<R>) -> AppResult<Config> {
    let store = app.store(STORE_FILE)?;
    let mut map = Map::new();
    for key in KEYS {
        if let Some(value) = store.get(key) {
            map.insert(key.to_string(), value);
        }
    }
    Ok(serde_json::from_value(Value::Object(map))?)
}

/// Flush a config snapshot to disk.
fn persist<R: Runtime>(app: &AppHandle<R>, config: &Config) -> AppResult<()> {
    let store = app.store(STORE_FILE)?;
    let Value::Object(map) = serde_json::to_value(config)? else {
        return Err(AppError::Internal("config did not serialize to an object".into()));
    };
    for (key, value) in map {
        store.set(key, value);
    }
    store.save()?;
    Ok(())
}

/// Read-only snapshot of the in-memory mirror.
pub fn get<R: Runtime>(app: &AppHandle<R>) -> Config {
    let state = app.state::<AppState>();
    let guard = state.config.read().expect("config lock poisoned");
    guard.clone()
}

/// Mutate the mirror and persist it atomically w.r.t. other callers: the lock
/// is held across the disk write so a concurrent `update` can't interleave a
/// stale snapshot. `f`'s return value is passed through.
///
/// The closure must not block for long — it runs under the write lock.
pub fn update<R, T, F>(app: &AppHandle<R>, f: F) -> AppResult<T>
where
    R: Runtime,
    F: FnOnce(&mut Config) -> AppResult<T>,
{
    let state = app.state::<AppState>();
    let mut guard = state.config.write().expect("config lock poisoned");
    let out = f(&mut guard)?;
    persist(app, &guard)?;
    Ok(out)
}
