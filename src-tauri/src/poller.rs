//! Per-device status poller, rebased onto Luci.
//!
//! One tokio task per device (never a single loop over all of them), so a dead
//! speaker costs only its own timeout. Each task owns:
//!
//! * one **persistent `LuciClient`** with `REG_ASYNC_EVENTS` — reconnected with
//!   5/10/30 s backoff when it drops;
//! * a periodic read of VOLUME / PLAY_STATE / CURRSOURCE / TRACK_INFO, plus a
//!   DDMS M-SEARCH for topology (State / NETMODE / model);
//! * its own failure streak — offline after 3 consecutive failed cycles.
//!
//! The tasks are the only writers of the snapshot cache; commands read it and
//! the frontend mirrors it through `device-updated` / `device-offline`.

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use serde_json::{json, Map, Value};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

use crate::luci::client::LuciClient;
use crate::luci::messagebox::MessageBox;
use crate::luci::model::{self, DdmsBanner, DevInfo, DeviceSnapshot, NetMode, Role, Track};
use crate::state::AppState;
use crate::store;

/// Snapshot changed (including the offline transition).
pub const EVENT_DEVICE_UPDATED: &str = "device-updated";
/// A device just crossed the offline threshold — emitted once per transition.
pub const EVENT_DEVICE_OFFLINE: &str = "device-offline";

/// Consecutive failed cycles before a device is called offline.
const OFFLINE_AFTER_FAILURES: u32 = 3;
/// Reconnect/retry cadence once offline.
const OFFLINE_BACKOFF: [u64; 3] = [5_000, 10_000, 30_000];
/// Floor for the poll interval.
const MIN_POLL_MS: u64 = 1_000;
/// Adaptive cadence: snappier while the window is focused, relaxed when it is
/// not (NFR — no point polling a window the user isn't looking at).
const FOCUSED_POLL_MS: u64 = 2_000;
const BLURRED_POLL_MS: u64 = 5_000;
/// DDMS topology probe budget per cycle.
const DDMS_TIMEOUT: Duration = Duration::from_millis(1_500);
/// UPnP now-playing budget per cycle (only spent while a device is playing).
const NOW_PLAYING_TIMEOUT: Duration = Duration::from_millis(1_500);

/// Unix milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct PollHandle {
    task: JoinHandle<()>,
    kick: std::sync::Arc<Notify>,
}

/// Owns the poll tasks and the last-known state of every device.
pub struct Poller {
    tasks: Mutex<HashMap<String, PollHandle>>,
    snapshots: RwLock<HashMap<String, DeviceSnapshot>>,
    /// Window focus, driving the adaptive interval. Starts focused (the window
    /// is up when the app launches).
    focused: std::sync::atomic::AtomicBool,
}

impl Default for Poller {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            snapshots: RwLock::new(HashMap::new()),
            focused: std::sync::atomic::AtomicBool::new(true),
        }
    }
}

impl Poller {
    pub fn snapshot(&self, uuid: &str) -> Option<DeviceSnapshot> {
        self.snapshots.read().expect("snapshot lock poisoned").get(uuid).cloned()
    }

    /// The poll interval for the current focus state.
    pub fn poll_interval_ms(&self) -> u64 {
        let ms = if self.focused.load(std::sync::atomic::Ordering::Relaxed) {
            FOCUSED_POLL_MS
        } else {
            BLURRED_POLL_MS
        };
        ms.max(MIN_POLL_MS)
    }

    /// Record window focus/blur. Kicks every loop so the new cadence takes hold
    /// at once rather than after the current (possibly 5 s) sleep.
    pub fn set_focused(&self, focused: bool) {
        let prev = self.focused.swap(focused, std::sync::atomic::Ordering::Relaxed);
        if prev != focused {
            self.kick_all();
        }
    }

    /// Wake every device's loop now.
    pub fn kick_all(&self) {
        for handle in self.tasks.lock().expect("poll task lock poisoned").values() {
            handle.kick.notify_one();
        }
    }

    pub fn put(&self, snapshot: DeviceSnapshot) {
        self.snapshots
            .write()
            .expect("snapshot lock poisoned")
            .insert(snapshot.uuid.clone(), snapshot);
    }

    fn forget(&self, uuid: &str) {
        self.snapshots.write().expect("snapshot lock poisoned").remove(uuid);
    }

    /// Start polling a device. Idempotent: an existing task is replaced.
    pub fn start(&self, app: &AppHandle, uuid: String, ip: String) {
        self.stop_task(&uuid);
        let kick = std::sync::Arc::new(Notify::new());
        let task = tauri::async_runtime::spawn(run(app.clone(), uuid.clone(), ip, kick.clone()));
        self.tasks
            .lock()
            .expect("poll task lock poisoned")
            .insert(uuid, PollHandle { task, kick });
    }

    /// Stop polling and drop the cached snapshot.
    pub fn stop(&self, uuid: &str) {
        self.stop_task(uuid);
        self.forget(uuid);
    }

    fn stop_task(&self, uuid: &str) {
        if let Some(handle) = self.tasks.lock().expect("poll task lock poisoned").remove(uuid) {
            handle.task.abort();
        }
    }

    /// Wake a device's loop now (manual refresh). False when unknown.
    pub fn kick(&self, uuid: &str) -> bool {
        match self.tasks.lock().expect("poll task lock poisoned").get(uuid) {
            Some(handle) => {
                handle.kick.notify_one();
                true
            }
            None => false,
        }
    }
}

/// A device's live playback read (via Luci) plus its DDMS banner.
pub(crate) struct Reading {
    pub(crate) volume: Option<u8>,
    pub(crate) mute: bool,
    pub(crate) source: Option<i32>,
    pub(crate) play_state: Option<i32>,
    pub(crate) track: Option<Track>,
    pub(crate) banner: Option<DdmsBanner>,
    pub(crate) raw: Map<String, Value>,
}

/// One periodic read against a connected device. Any hard Luci failure bubbles
/// up (the caller drops the client and reconnects); soft failures degrade the
/// affected field to `None`.
pub(crate) async fn read_once(client: &LuciClient, ip: &str, position_ms: Option<u64>) -> crate::error::AppResult<Reading> {
    let mut raw = Map::new();

    // VOLUME is the liveness probe — a failure here means the connection is bad.
    let volume_raw = client.read(MessageBox::Volume).await?;
    raw.insert("volume".into(), json!(volume_raw));
    let volume = model::parse_volume(&volume_raw);

    let mute = match client.read(MessageBox::MuteUnmute).await {
        Ok(p) => {
            raw.insert("mute".into(), json!(p));
            model::parse_mute(&p)
        }
        Err(e) => {
            log::debug!("{ip}: mute read failed: {e}");
            false
        }
    };

    let play_state = client.read(MessageBox::PlayState).await.ok().and_then(|p| {
        raw.insert("playState".into(), json!(p));
        model::parse_play_state(&p)
    });

    let source = client.read(MessageBox::CurrSource).await.ok().and_then(|p| {
        raw.insert("currSource".into(), json!(p));
        model::parse_source(&p)
    });

    // Now-playing. Luci `TRACK_INFO(44)` does not answer on this firmware (it
    // times out even mid-playback), so metadata comes from UPnP GetPositionInfo
    // instead — and only while actually playing, so an idle device never pays a
    // UPnP round trip. Best-effort: an unreachable renderer degrades to `None`.
    let mut track = None;
    if play_state == Some(1) {
        if let Ok(Ok(np)) = tokio::time::timeout(NOW_PLAYING_TIMEOUT, crate::upnp::now_playing(ip)).await {
            if !np.is_empty() {
                raw.insert("nowPlaying".into(), json!({
                    "title": np.title, "artist": np.artist, "album": np.album,
                    "durationMs": np.duration_ms, "positionMs": np.position_ms,
                }));
                track = Some(Track {
                    title: np.title,
                    artist: np.artist,
                    album: np.album,
                    duration_ms: np.duration_ms,
                    // Prefer the UPnP RelTime, falling back to the Luci push.
                    position_ms: np.position_ms.or(position_ms),
                });
            }
        }
    }
    // If UPnP gave nothing but a duration push landed, still surface a position.
    if track.is_none() && position_ms.is_some() {
        track = Some(Track { position_ms, ..Track::default() });
    }

    // DDMS banner for topology / netmode / model — best effort.
    let banner = crate::discovery::ddms_probe(ip, DDMS_TIMEOUT).await.map(|text| {
        raw.insert("ddms".into(), json!(text));
        DdmsBanner::parse(&text)
    });

    Ok(Reading { volume, mute, source, play_state, track, banner, raw })
}

/// Assemble a snapshot from cached identity + a fresh reading. Pure, so it is
/// unit-testable.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_snapshot(
    uuid: &str,
    ip: &str,
    alias: Option<String>,
    dev_info: Option<&DevInfo>,
    dev_name: &str,
    reading: &Reading,
    last_seen: i64,
) -> DeviceSnapshot {
    let banner = reading.banner.as_ref();

    let name = banner
        .and_then(|b| b.device_name())
        .map(str::to_string)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| dev_name.trim().to_string());

    let firmware = dev_info
        .map(|i| i.versioninfo.devicefwversion.clone())
        .filter(|f| !f.is_empty())
        .or_else(|| banner.and_then(|b| b.firmware()).map(str::to_string))
        .unwrap_or_default();

    let model = banner.and_then(|b| b.model()).map(str::to_string).unwrap_or_default();
    let net_mode = banner.and_then(|b| b.net_mode());
    let wifi_band = banner.and_then(|b| b.wifi_band()).map(str::to_string);
    let role = banner.map(DdmsBanner::role).unwrap_or(Role::Solo);
    let group_id = banner.and_then(|b| b.get("USN")).map(str::to_string).filter(|_| role != Role::Solo);

    let mut raw = reading.raw.clone();
    if let Some(info) = dev_info {
        raw.insert("devInfo".into(), serde_json::to_value(info).unwrap_or(Value::Null));
    }

    let mut snapshot = DeviceSnapshot {
        uuid: uuid.to_string(),
        ip: ip.to_string(),
        name,
        alias,
        display_name: String::new(),
        online: true,
        net_mode,
        wifi_band,
        model,
        firmware,
        role,
        group_id,
        master_uuid: None,
        volume: reading.volume,
        mute: reading.mute,
        source: reading.source,
        source_label: reading.source.map(model::source_label),
        play_state: reading.play_state,
        track: reading.track.clone(),
        last_seen: Some(last_seen),
        raw,
    };
    snapshot.refresh_display_name();
    snapshot
}

/// The per-device loop. Owns the connection, the failure streak and the backoff.
async fn run(app: AppHandle, uuid: String, ip: String, kick: std::sync::Arc<Notify>) {
    let mut client: Option<LuciClient> = None;
    let mut events: Option<tokio::sync::mpsc::Receiver<crate::luci::LuciEvent>> = None;
    let mut dev_info: Option<DevInfo> = None;
    let mut dev_name = String::new();
    let mut position_ms: Option<u64> = None;

    let mut failures = 0u32;
    let mut backoff = 0usize;
    let mut published: Option<DeviceSnapshot> = None;

    loop {
        let config = store::get(&app);
        let saved = config.devices.iter().find(|d| d.uuid == uuid);
        let alias = saved.and_then(|d| d.alias.clone());
        // Adaptive cadence, never faster than the user's configured floor.
        let floor = config.settings.poll_ms.clamp(store::MIN_POLL_MS, store::MAX_POLL_MS);
        let poll_ms = app.state::<AppState>().poller.poll_interval_ms().max(floor);

        // (Re)connect if needed. A fresh connection reloads the cached identity.
        if client.is_none() {
            match LuciClient::connect(&ip).await {
                Ok((c, rx)) => {
                    dev_info = c.read(MessageBox::DevInfo).await.ok().and_then(|p| DevInfo::parse(&p));
                    dev_name = c.read(MessageBox::DevName).await.unwrap_or_default().trim().to_string();
                    client = Some(c);
                    events = Some(rx);
                }
                Err(e) => {
                    log::debug!("{ip}: Luci connect failed ({failures}/{OFFLINE_AFTER_FAILURES}): {e}");
                }
            }
        }

        // Drain any pushed events (update the play position).
        if let Some(rx) = events.as_mut() {
            while let Ok((mb, _status, payload)) = rx.try_recv() {
                if mb == MessageBox::GetPlayDuration {
                    if let Ok(ms) = payload.trim().parse::<u64>() {
                        position_ms = Some(ms);
                    }
                }
            }
        }

        let wait = match client.as_ref() {
            Some(c) => match read_once(c, &ip, position_ms).await {
                Ok(reading) => {
                    failures = 0;
                    backoff = 0;
                    let net_mode = reading.banner.as_ref().and_then(|b| b.net_mode());
                    let snapshot = build_snapshot(&uuid, &ip, alias, dev_info.as_ref(), &dev_name, &reading, now_ms());
                    persist_seen(&app, &uuid, net_mode);
                    publish(&app, &mut published, snapshot, false);
                    poll_ms
                }
                Err(e) => {
                    log::debug!("{ip}: read failed: {e}");
                    // Drop the connection; the next cycle reconnects.
                    client = None;
                    events = None;
                    fail(&app, &uuid, &ip, alias, &mut published, &mut failures, &mut backoff)
                }
            },
            None => fail(&app, &uuid, &ip, alias, &mut published, &mut failures, &mut backoff),
        };

        tokio::select! {
            _ = kick.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
        }
    }
}

/// Record a failed cycle; publish offline once the threshold is crossed. Returns
/// the wait before the next attempt.
fn fail(
    app: &AppHandle,
    uuid: &str,
    ip: &str,
    alias: Option<String>,
    published: &mut Option<DeviceSnapshot>,
    failures: &mut u32,
    backoff: &mut usize,
) -> u64 {
    *failures += 1;
    if *failures < OFFLINE_AFTER_FAILURES {
        return MIN_POLL_MS.max(2_000);
    }
    let mut snapshot = published
        .clone()
        .unwrap_or_else(|| DeviceSnapshot::offline(uuid, ip, alias, None));
    snapshot.mark_offline();
    let was_online = published.as_ref().is_some_and(|p| p.online);
    publish(app, published, snapshot, true);
    if was_online {
        persist_seen(app, uuid, None);
    }
    let step = (*backoff).min(OFFLINE_BACKOFF.len() - 1);
    *backoff = (*backoff + 1).min(OFFLINE_BACKOFF.len() - 1);
    OFFLINE_BACKOFF[step]
}

/// Cache and emit — but only when something actually changed.
fn publish(app: &AppHandle, published: &mut Option<DeviceSnapshot>, snapshot: DeviceSnapshot, went_offline: bool) {
    let changed = published.as_ref() != Some(&snapshot);
    app.state::<AppState>().poller.put(snapshot.clone());
    if !changed {
        return;
    }
    if let Err(e) = app.emit(EVENT_DEVICE_UPDATED, &snapshot) {
        log::error!("failed to emit {EVENT_DEVICE_UPDATED}: {e}");
    }
    if went_offline {
        if let Err(e) = app.emit(EVENT_DEVICE_OFFLINE, &snapshot) {
            log::error!("failed to emit {EVENT_DEVICE_OFFLINE}: {e}");
        }
    }
    *published = Some(snapshot);
}

/// Persist last_seen (and net_mode when known), only on a real change, to avoid
/// hammering settings.json.
fn persist_seen(app: &AppHandle, uuid: &str, net_mode: Option<NetMode>) {
    let seen = now_ms();
    let mode = net_mode.map(|m| match m {
        NetMode::Ethernet => "ethernet".to_string(),
        NetMode::Wifi => "wifi".to_string(),
    });
    let result = store::update(app, |config| {
        if let Some(device) = config.devices.iter_mut().find(|d| d.uuid == uuid) {
            device.last_seen = Some(seen);
            if mode.is_some() && device.net_mode != mode {
                device.net_mode = mode.clone();
            }
        }
        Ok(())
    });
    if let Err(e) = result {
        log::error!("failed to persist last_seen for {uuid}: {e}");
    }
}

/// Start a poll task for every saved device (FR-6: re-poll on startup).
pub fn start_saved(app: &AppHandle) {
    let state = app.state::<AppState>();
    for device in store::get(app).devices {
        if device.uuid.is_empty() || device.ip.is_empty() {
            log::warn!("skipping malformed saved device: {device:?}");
            continue;
        }
        state.poller.put(DeviceSnapshot::offline(
            &device.uuid,
            &device.ip,
            device.alias.clone(),
            device.last_seen,
        ));
        state.poller.start(app, device.uuid, device.ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading_with_banner(banner_text: Option<&str>) -> Reading {
        Reading {
            volume: Some(30),
            mute: false,
            source: Some(10),
            play_state: Some(1),
            track: None,
            banner: banner_text.map(DdmsBanner::parse),
            raw: Map::new(),
        }
    }

    #[test]
    fn build_snapshot_prefers_ddms_name_and_devinfo_firmware() {
        let info: DevInfo = serde_json::from_str(
            r#"{"versioninfo":{"devicefwversion":"AR241CE_9243.16.2","mcuversion":"16"}}"#,
        )
        .unwrap();
        let reading = reading_with_banner(Some(
            "DeviceName:Lofficina-main\r\nState:S\r\nNETMODE:ETH0\r\nWIFIBAND:ETH\r\nCAST_MODEL:LP10\r\n",
        ));
        let snap = build_snapshot("U1", "192.168.10.104", None, Some(&info), "fallback", &reading, 42);
        assert_eq!(snap.uuid, "U1");
        assert_eq!(snap.name, "Lofficina-main");
        assert_eq!(snap.display_name, "Lofficina-main");
        assert_eq!(snap.firmware, "AR241CE_9243.16.2");
        assert_eq!(snap.model, "LP10");
        assert_eq!(snap.net_mode, Some(NetMode::Ethernet));
        assert_eq!(snap.wifi_band.as_deref(), Some("ETH"));
        assert_eq!(snap.role, Role::Solo);
        assert_eq!(snap.volume, Some(30));
        assert!(snap.online);
    }

    #[test]
    fn build_snapshot_alias_wins_display_name() {
        let reading = reading_with_banner(None);
        let snap = build_snapshot("U1", "1.2.3.4", Some("Cucina".into()), None, "LP10", &reading, 1);
        assert_eq!(snap.display_name, "Cucina");
        assert_eq!(snap.name, "LP10", "device name still recorded");
    }
}
