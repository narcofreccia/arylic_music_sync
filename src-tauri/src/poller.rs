//! Per-device status poller (brief §7, FR-6/FR-18/FR-19).
//!
//! One tokio task per device, never a single loop over all of them: NFR-3 wants
//! a dead speaker to cost only its own 2 s timeout, not to stall everyone
//! else's cycle. Each task owns its failure count and its backoff.
//!
//! The tasks are the only writers of the snapshot cache; commands read it, and
//! the frontend mirrors it through `device-updated` / `device-offline` events.
//! M5's Group Guard hooks in here (role changes are already computed per cycle).

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tauri::async_runtime::JoinHandle;
use tokio::sync::Notify;

use crate::error::AppResult;
use crate::linkplay::client::{LinkplayClient, LinkplayCommand};
use crate::linkplay::models::{
    derive_role, DeviceRole, DeviceSnapshot, PlayerStatus, SlaveList, StatusEx,
};
use crate::state::AppState;
use crate::store;

/// Snapshot changed (including the offline transition).
pub const EVENT_DEVICE_UPDATED: &str = "device-updated";
/// A device just crossed the offline threshold — a notification-worthy edge,
/// emitted once per transition rather than every failed cycle.
pub const EVENT_DEVICE_OFFLINE: &str = "device-offline";

/// Consecutive failed cycles before a device is called offline. A single miss
/// is normal on Wi-Fi (a stream burst, a retransmit) and must not flap the UI.
const OFFLINE_AFTER_FAILURES: u32 = 3;

/// Retry cadence once a device is offline — polling a dead unit every 3 s is
/// pure noise. Reset to the normal interval on the first success.
const OFFLINE_BACKOFF: [u64; 3] = [5_000, 10_000, 30_000];

/// Floor for the configurable interval; below this the LAN chatter outweighs
/// any responsiveness gain.
const MIN_POLL_MS: u64 = 1_000;

/// One device's poll round.
pub struct PollResult {
    pub snapshot: DeviceSnapshot,
    pub status: StatusEx,
    pub player: Option<PlayerStatus>,
}

/// A single poll round against one device.
///
/// Which calls are made depends on the role we can already infer, so a slave
/// costs one request instead of three: its transport is the master's, and it
/// has no slave list of its own.
pub async fn poll_once(
    client: &LinkplayClient,
    ip: &str,
    alias: Option<String>,
) -> AppResult<PollResult> {
    let status: StatusEx = client.send_json(ip, &LinkplayCommand::GetStatusEx).await?;

    // Provisional role from getStatusEx alone: enough to know whether the extra
    // two calls are worth making.
    let following = matches!(derive_role(&status, None, None), DeviceRole::Slave { .. });

    let player = if following {
        None
    } else {
        // A device that answers getStatusEx but chokes on getPlayerStatus is
        // still online — degrade to "no playback info" instead of offline.
        client
            .send_json::<PlayerStatus>(ip, &LinkplayCommand::GetPlayerStatus)
            .await
            .map_err(|e| log::debug!("{ip}: getPlayerStatus failed: {e}"))
            .ok()
    };

    let slaves = if following {
        None
    } else {
        client
            .send_json::<SlaveList>(ip, &LinkplayCommand::GetSlaveList)
            .await
            .map_err(|e| log::debug!("{ip}: getSlaveList failed: {e}"))
            .ok()
    };

    let snapshot = DeviceSnapshot::build(ip, alias, &status, player.as_ref(), slaves.as_ref(), now_ms());
    Ok(PollResult { snapshot, status, player })
}

/// Unix milliseconds; `0` if the clock is before the epoch (it isn't).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct PollHandle {
    task: JoinHandle<()>,
    /// Wakes the loop early (`refresh_device`, or after a rename).
    kick: std::sync::Arc<Notify>,
}

/// Owns the poll tasks and the last-known state of every device. Lives in
/// `AppState`.
#[derive(Default)]
pub struct Poller {
    tasks: Mutex<HashMap<String, PollHandle>>,
    snapshots: RwLock<HashMap<String, DeviceSnapshot>>,
}

impl Poller {
    pub fn snapshot(&self, uuid: &str) -> Option<DeviceSnapshot> {
        self.snapshots.read().expect("snapshot lock poisoned").get(uuid).cloned()
    }

    /// Publish a snapshot without going through a poll cycle (`add_device`
    /// already has a fresh one).
    pub fn put(&self, snapshot: DeviceSnapshot) {
        self.snapshots
            .write()
            .expect("snapshot lock poisoned")
            .insert(snapshot.uuid.clone(), snapshot);
    }

    fn forget(&self, uuid: &str) {
        self.snapshots.write().expect("snapshot lock poisoned").remove(uuid);
    }

    /// Start polling a device. Idempotent: an existing task for `uuid` is
    /// replaced, so a re-added device never ends up with two loops.
    pub fn start(&self, app: &AppHandle, uuid: String, ip: String) {
        self.stop(&uuid);

        let kick = std::sync::Arc::new(Notify::new());
        let task = tauri::async_runtime::spawn(run(app.clone(), uuid.clone(), ip, kick.clone()));
        self.tasks
            .lock()
            .expect("poll task lock poisoned")
            .insert(uuid, PollHandle { task, kick });
    }

    /// Stop polling and drop the cached snapshot.
    pub fn stop(&self, uuid: &str) {
        if let Some(handle) = self.tasks.lock().expect("poll task lock poisoned").remove(uuid) {
            handle.task.abort();
        }
        self.forget(uuid);
    }

    /// Wake a device's loop now (FR-6 manual refresh). False when unknown.
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

/// The per-device loop. Owns its failure streak and backoff so devices fail
/// independently (NFR-3).
async fn run(app: AppHandle, uuid: String, ip: String, kick: std::sync::Arc<Notify>) {
    let client = app.state::<AppState>().linkplay.clone();
    let mut failures = 0u32;
    let mut backoff = 0usize;
    // The last state we told the frontend about — the emit filter.
    let mut published: Option<DeviceSnapshot> = None;

    loop {
        let config = store::get(&app);
        let alias = config
            .devices
            .iter()
            .find(|d| d.uuid == uuid)
            .and_then(|d| d.alias.clone());
        let poll_ms = config.settings.poll_ms.max(MIN_POLL_MS);

        let wait = match poll_once(&client, &ip, alias).await {
            Ok(result) => {
                failures = 0;
                backoff = 0;

                let mut snapshot = result.snapshot;
                if snapshot.uuid != uuid {
                    // The saved UUID is this device's identity everywhere else
                    // (config key, map key, event key). A mismatch means the IP
                    // was reassigned to another unit — keep the key stable and
                    // let the user notice the changed name.
                    if !snapshot.uuid.is_empty() {
                        log::warn!("{ip} reports uuid {} but is saved as {uuid}", snapshot.uuid);
                    }
                    snapshot.uuid = uuid.clone();
                }

                publish(&app, &mut published, snapshot, false);
                poll_ms
            }
            Err(e) => {
                failures += 1;
                log::debug!("{ip}: poll failed ({failures}/{OFFLINE_AFTER_FAILURES}): {e}");

                if failures >= OFFLINE_AFTER_FAILURES {
                    // Keep the last-known identity fields so the card stays
                    // recognisable while it is greyed out.
                    let mut snapshot = published
                        .clone()
                        .unwrap_or_else(|| DeviceSnapshot::offline(&uuid, &ip, None, None));
                    snapshot.mark_offline();
                    let was_online = published.as_ref().is_some_and(|p| p.online);
                    publish(&app, &mut published, snapshot, true);

                    if was_online {
                        // Persist the last sighting once, on the edge — writing
                        // it every cycle would hammer settings.json.
                        persist_last_seen(&app, &uuid);
                    }
                    let step = backoff.min(OFFLINE_BACKOFF.len() - 1);
                    backoff = (backoff + 1).min(OFFLINE_BACKOFF.len() - 1);
                    OFFLINE_BACKOFF[step]
                } else {
                    poll_ms
                }
            }
        };

        tokio::select! {
            _ = kick.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
        }
    }
}

/// Cache the snapshot and emit — but only when something actually changed.
/// `DeviceSnapshot: PartialEq` is the diff; it is cheaper and exact where a
/// serialized hash would only be an approximation of the same test.
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

fn persist_last_seen(app: &AppHandle, uuid: &str) {
    let seen = now_ms();
    let result = store::update(app, |config| {
        if let Some(device) = config.devices.iter_mut().find(|d| d.uuid == uuid) {
            device.last_seen = Some(seen);
        }
        Ok(())
    });
    if let Err(e) = result {
        log::error!("failed to persist last_seen for {uuid}: {e}");
    }
}

/// Start a poll task for every saved device (FR-6: re-poll known devices on
/// startup). Called from `run()`'s setup, after the config is loaded.
pub fn start_saved(app: &AppHandle) {
    let state = app.state::<AppState>();
    for device in store::get(app).devices {
        if device.uuid.is_empty() || device.ip.is_empty() {
            log::warn!("skipping malformed saved device: {device:?}");
            continue;
        }
        // Render the persisted entry immediately; the first cycle replaces it.
        state.poller.put(DeviceSnapshot::offline(
            &device.uuid,
            &device.ip,
            device.alias.clone(),
            device.last_seen,
        ));
        state.poller.start(app, device.uuid, device.ip);
    }
}
