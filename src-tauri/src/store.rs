//! Persisted configuration — a single `settings.json` in the app config dir,
//! managed by tauri-plugin-store.
//!
//! The whole file is mirrored in memory (`AppState::config`) so reads are free;
//! every mutation goes through [`update`], which writes the mirror and flushes
//! to disk in one step. Top-level `Config` fields map 1:1 to store keys, so the
//! on-disk JSON stays readable/hand-editable.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_store::StoreExt;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub const STORE_FILE: &str = "settings.json";

/// Store keys we own. Anything else in the file is left untouched.
const KEYS: [&str; 6] = [
    "auth",
    "devices",
    "settings",
    "remember_me",
    "manual_targets",
    "device_delays",
];

/// Default RAOP/AirPlay-1 control port a manual speaker listens on.
pub const DEFAULT_RAOP_PORT: u16 = 5000;

/// Upper bound for a persisted per-device playback delay (ms). Wide enough to
/// trim any realistic room-to-room skew; a hand-edited config can't drive the
/// silent lead-in past this.
pub const MAX_DEVICE_DELAY_MS: u32 = 2_000;

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
///
/// `guard_mode` / `failover_mode` are carried for on-disk back-compat only: the
/// LP10's native DDMS grouping is non-functional (docs/firmware-notes.md §G/§H),
/// so there is no Group Guard or master failover and the UI never surfaces them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Poll-interval floor in ms. The poller runs an adaptive 2 s/5 s cadence
    /// (focused/blurred); this is the user's override — the effective interval
    /// is never faster than this.
    pub poll_ms: u64,
    /// CIDR for the subnet sweep; `None` = auto-detect from local interfaces.
    pub subnet: Option<String>,
    /// UI theme: "dark" | "light" | "system".
    pub theme: String,
    /// Per-request network budget in ms (Luci/UPnP round trips).
    pub http_timeout_ms: u64,
    /// Launch MusicSync at login (wired to tauri-plugin-autostart).
    pub start_at_login: bool,
    /// Deprecated — grouping is unsupported on LP10. Kept for config back-compat.
    #[serde(default = "default_guard_mode")]
    pub guard_mode: String,
    /// Deprecated — grouping is unsupported on LP10. Kept for config back-compat.
    #[serde(default = "default_failover_mode")]
    pub failover_mode: String,
}

/// Poll-interval floor bounds. Below `MIN` the LAN is hammered for nothing;
/// above `MAX` the UI feels dead.
pub const MIN_POLL_MS: u64 = 1_000;
pub const MAX_POLL_MS: u64 = 60_000;
/// Per-request timeout bounds.
pub const MIN_HTTP_TIMEOUT_MS: u64 = 500;
pub const MAX_HTTP_TIMEOUT_MS: u64 = 30_000;
/// The themes the UI can render; anything else falls back to "dark".
pub const THEMES: [&str; 3] = ["dark", "light", "system"];

fn default_guard_mode() -> String {
    "ask".into()
}
fn default_failover_mode() -> String {
    "prompt".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_ms: 3000,
            subnet: None,
            theme: "dark".into(),
            http_timeout_ms: 4000,
            start_at_login: false,
            guard_mode: default_guard_mode(),
            failover_mode: default_failover_mode(),
        }
    }
}

impl Settings {
    /// Clamp numeric fields into range and normalise the theme. Applied on every
    /// mutation so a hand-edited or imported config can never drive the app into
    /// a pathological state (a 0 ms poll, an unknown theme).
    pub fn sanitize(&mut self) {
        self.poll_ms = self.poll_ms.clamp(MIN_POLL_MS, MAX_POLL_MS);
        self.http_timeout_ms = self.http_timeout_ms.clamp(MIN_HTTP_TIMEOUT_MS, MAX_HTTP_TIMEOUT_MS);
        if !THEMES.contains(&self.theme.as_str()) {
            self.theme = "dark".into();
        }
    }
}

/// A manually-added RAOP receiver (Feature 1). Unlike a [`SavedDevice`] it has no
/// DDMS/UPnP identity — it is just a name + `ip:port` the user typed, so the
/// "Play Everywhere" flow can be exercised against any AirPlay/RAOP receiver
/// (crucially a local `shairport-sync` instance) without real LP10 hardware.
///
/// Its `id` is the stable key: it survives an IP/port edit and keys the
/// per-device delay map, exactly as a device's UUID does for discovered speakers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ManualTarget {
    /// Stable local id (`manual-<hex>`), independent of ip/port.
    pub id: String,
    /// Human label (also the shairport-sync `-a` name on the rig).
    pub name: String,
    /// Receiver IP (`127.0.0.1` for a local test receiver).
    pub ip: String,
    /// RAOP control port (`5000` default; use another if the Mac's AirPlay
    /// Receiver already holds 5000).
    pub port: u16,
}

impl Default for ManualTarget {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            ip: String::new(),
            port: DEFAULT_RAOP_PORT,
        }
    }
}

impl ManualTarget {
    /// Validate user input and mint a fresh manual target. The name must be
    /// non-empty, the IP must parse (v4/v6; `127.0.0.1` for the local rig), and
    /// the port must be non-zero.
    pub fn create(name: &str, ip: &str, port: u16) -> AppResult<Self> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::InvalidInput("Give the speaker a name.".into()));
        }
        let ip = ip.trim();
        if ip.parse::<IpAddr>().is_err() {
            return Err(AppError::InvalidInput(format!(
                "“{ip}” is not a valid IP address."
            )));
        }
        if port == 0 {
            return Err(AppError::InvalidInput(
                "Port must be between 1 and 65535.".into(),
            ));
        }
        Ok(Self {
            id: new_manual_id(),
            name: name.to_string(),
            ip: ip.to_string(),
            port,
        })
    }
}

/// A random, collision-improbable id for a manual target.
fn new_manual_id() -> String {
    format!("manual-{:016x}", rand::random::<u64>())
}

/// Clamp a requested delay into the persisted range (`0..=MAX_DEVICE_DELAY_MS`).
pub fn clamp_delay(ms: u32) -> u32 {
    ms.min(MAX_DEVICE_DELAY_MS)
}

/// The persistence key for a stream target's delay: its UUID (discovered device)
/// or manual-target id when present, else its IP. Keeping the rule in one place
/// means the picker, the RoomRow and the engine all agree on the same key.
pub fn delay_key(uuid: Option<&str>, ip: &str) -> String {
    match uuid {
        Some(u) if !u.trim().is_empty() => u.to_string(),
        _ => ip.to_string(),
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
    /// Feature 1: manually-added RAOP receivers (test rig / non-LP10 speakers).
    pub manual_targets: Vec<ManualTarget>,
    /// Feature 2: persisted per-target playback delay in ms, keyed by
    /// [`delay_key`] (device UUID / manual-target id / IP). Absent = 0 ms.
    pub device_delays: BTreeMap<String, u32>,
}

impl Config {
    /// Add a validated manual target. Rejects a duplicate `ip:port` so the same
    /// receiver can't be added twice. Returns the newly-created target.
    pub fn add_manual_target(&mut self, name: &str, ip: &str, port: u16) -> AppResult<ManualTarget> {
        let target = ManualTarget::create(name, ip, port)?;
        if self
            .manual_targets
            .iter()
            .any(|m| m.ip == target.ip && m.port == target.port)
        {
            return Err(AppError::InvalidInput(format!(
                "A speaker at {}:{} is already added.",
                target.ip, target.port
            )));
        }
        self.manual_targets.push(target.clone());
        Ok(target)
    }

    /// Remove a manual target (and forget any delay saved under its id).
    pub fn remove_manual_target(&mut self, id: &str) {
        self.manual_targets.retain(|m| m.id != id);
        self.device_delays.remove(id);
    }

    /// Persist a per-target delay (clamped). A 0 ms delay is dropped from the map
    /// rather than stored, so the on-disk config stays minimal and the 0 ms path
    /// is the natural default. Returns the clamped value actually stored.
    pub fn set_target_delay(&mut self, key: &str, ms: u32) -> u32 {
        let ms = clamp_delay(ms);
        if ms == 0 {
            self.device_delays.remove(key);
        } else {
            self.device_delays.insert(key.to_string(), ms);
        }
        ms
    }

    /// The saved delay for a target key (0 when none).
    pub fn target_delay(&self, key: &str) -> u32 {
        self.device_delays.get(key).copied().unwrap_or(0)
    }
}

/// A portable slice of the config (FR-21): everything worth carrying between
/// machines — settings + saved devices — and, by construction, **nothing else**.
/// Auth (the Argon2 hash) and the "remember me" grant are not fields here, so an
/// export can never leak a credential and an import can never overwrite one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigBundle {
    pub settings: Settings,
    #[serde(default)]
    pub devices: Vec<SavedDevice>,
}

/// Build the export bundle. Pure — the command layer only adds file I/O.
pub fn export_bundle(config: &Config) -> ConfigBundle {
    ConfigBundle {
        settings: config.settings.clone(),
        devices: config.devices.clone(),
    }
}

/// Merge an imported bundle into `config` in place, preserving auth + remember_me
/// (those aren't in the bundle, so they're simply left alone). Settings are
/// replaced wholesale (then sanitised); devices are unioned by uuid — an imported
/// device updates a matching saved one, otherwise it's appended. Pure and
/// unit-tested. Returns the number of devices added or updated.
pub fn merge_bundle(config: &mut Config, bundle: ConfigBundle) -> usize {
    config.settings = bundle.settings;
    config.settings.sanitize();

    let mut touched = 0;
    for dev in bundle.devices {
        // A device with no identity at all is junk from a hand-edited file.
        if dev.uuid.is_empty() && dev.usn.is_empty() && dev.ip.is_empty() {
            continue;
        }
        match config.devices.iter_mut().find(|d| {
            (!dev.uuid.is_empty() && d.uuid == dev.uuid) || (!dev.usn.is_empty() && d.usn == dev.usn)
        }) {
            Some(existing) => *existing = dev,
            None => config.devices.push(dev),
        }
        touched += 1;
    }
    touched
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

#[cfg(test)]
mod tests {
    use super::*;

    fn device(uuid: &str, ip: &str) -> SavedDevice {
        SavedDevice {
            uuid: uuid.into(),
            ip: ip.into(),
            ..Default::default()
        }
    }

    #[test]
    fn sanitize_clamps_and_normalises() {
        let mut s = Settings {
            poll_ms: 0,
            http_timeout_ms: 999_999,
            theme: "solarized".into(),
            ..Settings::default()
        };
        s.sanitize();
        assert_eq!(s.poll_ms, MIN_POLL_MS);
        assert_eq!(s.http_timeout_ms, MAX_HTTP_TIMEOUT_MS);
        assert_eq!(s.theme, "dark", "unknown theme falls back to dark");

        let mut ok = Settings {
            poll_ms: 4000,
            http_timeout_ms: 2000,
            theme: "system".into(),
            ..Settings::default()
        };
        ok.sanitize();
        assert_eq!(ok.poll_ms, 4000);
        assert_eq!(ok.theme, "system", "valid theme is preserved");
    }

    #[test]
    fn export_bundle_omits_auth_and_remember_me() {
        let config = Config {
            auth: Some(AuthConfig {
                username: "andrea".into(),
                password_hash: Some("$argon2id$secret".into()),
            }),
            devices: vec![device("U1", "1.2.3.4")],
            settings: Settings::default(),
            remember_me: true,
        };
        let bundle = export_bundle(&config);
        // The serialized export must not carry the hash or the login grant.
        let json = serde_json::to_string(&bundle).unwrap();
        assert!(!json.contains("argon2"), "auth hash must be stripped");
        assert!(!json.contains("remember_me"), "login grant must not export");
        assert!(!json.contains("password_hash"));
        assert_eq!(bundle.devices.len(), 1);
    }

    #[test]
    fn merge_bundle_preserves_auth_and_unions_devices() {
        let mut config = Config {
            auth: Some(AuthConfig {
                username: "andrea".into(),
                password_hash: Some("$argon2id$keep-me".into()),
            }),
            devices: vec![device("U1", "1.1.1.1"), device("U2", "2.2.2.2")],
            settings: Settings::default(),
            remember_me: true,
        };
        let bundle = ConfigBundle {
            settings: Settings {
                theme: "light".into(),
                ..Settings::default()
            },
            // U1 moved IP (update in place); U3 is new (append).
            devices: vec![device("U1", "9.9.9.9"), device("U3", "3.3.3.3")],
        };
        let touched = merge_bundle(&mut config, bundle);
        assert_eq!(touched, 2);

        // Auth + remember_me untouched by the import.
        assert_eq!(config.auth.unwrap().password_hash.unwrap(), "$argon2id$keep-me");
        assert!(config.remember_me);

        // Settings replaced from the bundle.
        assert_eq!(config.settings.theme, "light");

        // Devices unioned by uuid, not duplicated.
        assert_eq!(config.devices.len(), 3);
        let u1 = config.devices.iter().find(|d| d.uuid == "U1").unwrap();
        assert_eq!(u1.ip, "9.9.9.9", "matching uuid updates in place");
        assert!(config.devices.iter().any(|d| d.uuid == "U3"));
    }

    #[test]
    fn merge_bundle_sanitises_imported_settings() {
        let mut config = Config::default();
        let bundle = ConfigBundle {
            settings: Settings {
                poll_ms: 0,
                theme: "neon".into(),
                ..Settings::default()
            },
            devices: vec![],
        };
        merge_bundle(&mut config, bundle);
        assert_eq!(config.settings.poll_ms, MIN_POLL_MS);
        assert_eq!(config.settings.theme, "dark");
    }

    #[test]
    fn manual_target_validation_rejects_bad_input() {
        assert!(ManualTarget::create("", "127.0.0.1", 5000).is_err(), "empty name");
        assert!(ManualTarget::create("Test", "not-an-ip", 5000).is_err(), "bad ip");
        assert!(ManualTarget::create("Test", "127.0.0.1", 0).is_err(), "zero port");
        let ok = ManualTarget::create("  Test Room  ", " 127.0.0.1 ", 5001).unwrap();
        assert_eq!(ok.name, "Test Room", "name is trimmed");
        assert_eq!(ok.ip, "127.0.0.1", "ip is trimmed");
        assert_eq!(ok.port, 5001);
        assert!(ok.id.starts_with("manual-"));
    }

    #[test]
    fn manual_target_crud_and_dedup() {
        let mut config = Config::default();
        let a = config.add_manual_target("Room A", "127.0.0.1", 5001).unwrap();
        config.add_manual_target("Room B", "127.0.0.1", 5002).unwrap();
        assert_eq!(config.manual_targets.len(), 2);

        // Same ip:port is rejected; a different port on the same ip is fine.
        assert!(config.add_manual_target("Dup", "127.0.0.1", 5001).is_err());
        assert_eq!(config.manual_targets.len(), 2);

        // Removing also forgets its saved delay.
        config.set_target_delay(&a.id, 120);
        assert_eq!(config.target_delay(&a.id), 120);
        config.remove_manual_target(&a.id);
        assert_eq!(config.manual_targets.len(), 1);
        assert_eq!(config.target_delay(&a.id), 0, "delay removed with the target");
    }

    #[test]
    fn delay_persistence_clamps_and_prunes() {
        let mut config = Config::default();
        // Clamped to the max.
        assert_eq!(config.set_target_delay("U1", 999_999), MAX_DEVICE_DELAY_MS);
        assert_eq!(config.target_delay("U1"), MAX_DEVICE_DELAY_MS);
        // A mid-range value round-trips.
        assert_eq!(config.set_target_delay("U1", 250), 250);
        assert_eq!(config.target_delay("U1"), 250);
        // Zero prunes the entry entirely.
        config.set_target_delay("U1", 0);
        assert!(!config.device_delays.contains_key("U1"), "0 ms is not stored");
        assert_eq!(config.target_delay("missing"), 0);
    }

    #[test]
    fn delay_key_prefers_uuid_then_ip() {
        assert_eq!(delay_key(Some("U1"), "1.2.3.4"), "U1");
        assert_eq!(delay_key(Some("manual-abc"), "127.0.0.1"), "manual-abc");
        assert_eq!(delay_key(None, "1.2.3.4"), "1.2.3.4");
        assert_eq!(delay_key(Some(""), "1.2.3.4"), "1.2.3.4", "blank uuid falls back to ip");
    }

    #[test]
    fn import_json_never_injects_auth() {
        // A crafted file with an `auth` key must be ignored: ConfigBundle has no
        // such field, so serde drops it.
        let json = r#"{"settings":{},"devices":[],"auth":{"username":"evil","password_hash":"x"}}"#;
        let bundle: ConfigBundle = serde_json::from_str(json).unwrap();
        let mut config = Config {
            auth: Some(AuthConfig { username: "me".into(), password_hash: Some("real".into()) }),
            ..Config::default()
        };
        merge_bundle(&mut config, bundle);
        assert_eq!(config.auth.unwrap().password_hash.unwrap(), "real");
    }
}
