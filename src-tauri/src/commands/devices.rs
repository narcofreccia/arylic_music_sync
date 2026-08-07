//! Device management commands (brief.md FR-5 … FR-9).
//!
//! These are the *user-initiated* device operations; continuous state comes
//! from the poller, which pushes `device-updated` / `device-offline` events. A
//! command therefore never returns "the list plus a promise to refresh" — it
//! returns what it knows now and lets the poller correct it a cycle later.

use std::net::Ipv4Addr;

use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};
use crate::linkplay::client::LinkplayCommand;
use crate::linkplay::models::{DeviceDetail, DeviceSnapshot};
use crate::poller::poll_once;
use crate::state::AppState;
use crate::store::{self, SavedDevice};

/// Look up a saved device's IP and alias, or fail with a stale-reference error.
fn saved(app: &AppHandle, uuid: &str) -> AppResult<SavedDevice> {
    store::get(app)
        .devices
        .into_iter()
        .find(|d| d.uuid == uuid)
        .ok_or_else(|| AppError::NotFound("That device is no longer in the list.".into()))
}

/// FR-5: add a device by IP.
///
/// The address is validated by *talking* to it, not by pattern-matching: a
/// reachable box that answers `getStatusEx` with a UUID is an LP10 as far as
/// this app is concerned. Nothing is persisted until that succeeds, so a typo
/// never leaves a phantom entry behind.
///
/// **Idempotent since M3.** Adding a UUID that is already saved refreshes its
/// IP instead of failing. FR-4's "add all" would otherwise have to filter the
/// already-added candidates *and* be right about it, and a speaker that took a
/// new DHCP lease would be un-re-addable — the very case where re-adding it by
/// its new address is exactly what the user means.
#[tauri::command]
pub async fn add_device(app: AppHandle, ip: String) -> AppResult<DeviceSnapshot> {
    let ip = ip.trim().to_string();
    if ip.parse::<Ipv4Addr>().is_err() {
        return Err(AppError::InvalidInput(format!(
            "“{ip}” is not a valid IPv4 address (for example 192.168.1.42)."
        )));
    }

    let client = app.state::<AppState>().linkplay.clone();
    let result = poll_once(&client, &ip, None).await?;
    let snapshot = result.snapshot;
    if snapshot.uuid.is_empty() {
        return Err(AppError::Device(format!(
            "{ip} answered, but did not report a UUID — it may not be a Linkplay device."
        )));
    }

    let uuid = snapshot.uuid.clone();
    let entry = SavedDevice {
        uuid: uuid.clone(),
        ip: ip.clone(),
        alias: None,
        last_seen: snapshot.last_seen,
        // FR-5 additions are pinned: M3's sweep must never prune them.
        pinned_manual: true,
    };
    store::update(&app, move |config| {
        // Identity is the UUID, so the same speaker reached on a new IP is an
        // update, not a duplicate.
        if let Some(existing) = config.devices.iter_mut().find(|d| d.uuid == entry.uuid) {
            if existing.ip != entry.ip {
                log::info!("{} moved from {} to {}", entry.uuid, existing.ip, entry.ip);
            }
            existing.ip = entry.ip.clone();
            existing.last_seen = entry.last_seen;
            existing.pinned_manual = true;
            return Ok(());
        }
        config.devices.push(entry);
        Ok(())
    })?;

    let state = app.state::<AppState>();
    state.poller.put(snapshot.clone());
    state.poller.start(&app, uuid, ip);
    Ok(snapshot)
}

/// FR-8: forget a device. Stops its poll task first so no in-flight cycle can
/// re-publish a snapshot for a device that is no longer in the list.
#[tauri::command]
pub fn remove_device(app: AppHandle, uuid: String) -> AppResult<()> {
    app.state::<AppState>().poller.stop(&uuid);
    store::update(&app, |config| {
        let before = config.devices.len();
        config.devices.retain(|d| d.uuid != uuid);
        if config.devices.len() == before {
            return Err(AppError::NotFound("That device is no longer in the list.".into()));
        }
        Ok(())
    })
}

/// FR-7: set a local alias, optionally pushing the name to the device itself.
///
/// The push happens first: if the device refuses the name we don't want a local
/// alias claiming a rename that never reached the hardware (and it is the name
/// Spotify Connect shows — FR-22 depends on it being true).
#[tauri::command]
pub async fn rename_device(
    app: AppHandle,
    uuid: String,
    alias: Option<String>,
    push_to_device: bool,
) -> AppResult<DeviceSnapshot> {
    let device = saved(&app, &uuid)?;
    // An empty alias means "go back to the device's own name".
    let alias = alias.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());

    if push_to_device {
        let name = alias.clone().ok_or_else(|| {
            AppError::InvalidInput("Enter a name before pushing it to the device.".into())
        })?;
        let client = app.state::<AppState>().linkplay.clone();
        client
            .send_ok(&device.ip, &LinkplayCommand::SetDeviceName(name))
            .await?;
    }

    let stored = alias.clone();
    store::update(&app, move |config| match config.devices.iter_mut().find(|d| d.uuid == uuid) {
        Some(d) => {
            d.alias = stored;
            Ok(())
        }
        None => Err(AppError::NotFound("That device is no longer in the list.".into())),
    })?;

    // Reflect the new name immediately; the kicked poll cycle then confirms it
    // (and picks up the pushed device name) without waiting a full interval.
    let state = app.state::<AppState>();
    let snapshot = match state.poller.snapshot(&device.uuid) {
        Some(mut snapshot) => {
            snapshot.alias = alias.clone();
            snapshot.display_name = alias
                .clone()
                .or_else(|| Some(snapshot.name.clone()).filter(|n| !n.trim().is_empty()))
                .unwrap_or_else(|| snapshot.ip.clone());
            snapshot
        }
        None => DeviceSnapshot::offline(&device.uuid, &device.ip, alias, device.last_seen),
    };
    state.poller.put(snapshot.clone());
    state.poller.kick(&device.uuid);
    Ok(snapshot)
}

/// FR-6: the persisted list, hydrated with whatever the poller last saw.
///
/// Devices never polled yet come back as offline placeholders rather than being
/// omitted — a cold start must render the user's list instantly, not an empty
/// page that fills in three seconds later.
#[tauri::command]
pub fn list_devices(app: AppHandle) -> Vec<DeviceSnapshot> {
    let state = app.state::<AppState>();
    store::get(&app)
        .devices
        .into_iter()
        .map(|device| match state.poller.snapshot(&device.uuid) {
            Some(snapshot) => snapshot,
            None => DeviceSnapshot::offline(&device.uuid, &device.ip, device.alias, device.last_seen),
        })
        .collect()
}

/// FR-9: the detail view — a *live* round trip, plus every field this firmware
/// sends that we don't model, for the debug pane and the FR-23 spike.
#[tauri::command]
pub async fn get_status(app: AppHandle, uuid: String) -> AppResult<DeviceDetail> {
    let device = saved(&app, &uuid)?;
    let client = app.state::<AppState>().linkplay.clone();
    let result = poll_once(&client, &device.ip, device.alias.clone()).await?;

    let mut snapshot = result.snapshot;
    // Keep the saved identity even if the box reports a different UUID; the
    // poll task logs that mismatch.
    snapshot.uuid = device.uuid.clone();
    app.state::<AppState>().poller.put(snapshot.clone());

    Ok(DeviceDetail {
        snapshot,
        extra: result.status.extra,
        player_extra: result.player.map(|p| p.extra).unwrap_or_default(),
    })
}

/// Wake a device's poll loop now instead of waiting out the interval. Starts a
/// task for a saved device that somehow has none (e.g. added before a restart
/// that failed to spawn it).
#[tauri::command]
pub fn refresh_device(app: AppHandle, uuid: String) -> AppResult<()> {
    let device = saved(&app, &uuid)?;
    let state = app.state::<AppState>();
    if !state.poller.kick(&device.uuid) {
        state.poller.start(&app, device.uuid, device.ip);
    }
    Ok(())
}

/// Best-effort LAN address of this machine — shown in the debug pane and used
/// by M3 to seed the subnet sweep.
#[tauri::command]
pub fn local_address() -> Option<String> {
    crate::net::local_ipv4().map(|ip| ip.to_string())
}
