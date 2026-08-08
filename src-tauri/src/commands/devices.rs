//! Device management commands (FR-5 … FR-9), rebased onto Luci.
//!
//! These are the *user-initiated* operations; continuous state comes from the
//! poller, which pushes `device-updated` / `device-offline`. A command returns
//! what it knows now and lets the poller correct it a cycle later.

use std::net::Ipv4Addr;

use tauri::{AppHandle, Manager};

use crate::discovery;
use crate::error::{AppError, AppResult};
use crate::luci::messagebox::{MessageBox, MessageType};
use crate::luci::model::{DevInfo, DeviceDetail, DeviceSnapshot, NetMode};
use crate::luci::LuciClient;
use crate::poller::{build_snapshot, now_ms, read_once};
use crate::state::AppState;
use crate::store::{self, SavedDevice};

/// Look up a saved device, or fail with a stale-reference error.
fn saved(app: &AppHandle, uuid: &str) -> AppResult<SavedDevice> {
    store::get(app)
        .devices
        .into_iter()
        .find(|d| d.uuid == uuid)
        .ok_or_else(|| AppError::NotFound("That device is no longer in the list.".into()))
}

/// Everything one live confirmation round yields.
struct Probe {
    snapshot: DeviceSnapshot,
    /// DDMS `USN` / eth0 MAC — the fallback identity.
    usn: String,
    /// UPnP UDN uuid (empty when description.xml was unreachable).
    upnp_uuid: String,
}

/// Connect over Luci, confirm the device (`DevInfo`), read its live state, and
/// gather the DDMS banner + UPnP uuid. The `key` is the identity the snapshot is
/// stamped with (the saved uuid on refresh; computed by `add_device` on first
/// contact).
async fn probe(ip: &str, key: &str, alias: Option<String>) -> AppResult<Probe> {
    let ipv4: Ipv4Addr = ip
        .parse()
        .map_err(|_| AppError::InvalidInput(format!("“{ip}” is not a valid IPv4 address.")))?;

    let (client, _events) = LuciClient::connect(ip).await?;

    let dev_info: DevInfo = client
        .read(MessageBox::DevInfo)
        .await
        .ok()
        .and_then(|p| DevInfo::parse(&p))
        .ok_or_else(|| {
            AppError::Device(format!("{ip} answered on Luci but did not return DevInfo — it may not be an LP10."))
        })?;
    let dev_name = client.read(MessageBox::DevName).await.unwrap_or_default().trim().to_string();

    let reading = read_once(&client, ip, None).await?;

    let usn = reading
        .banner
        .as_ref()
        .and_then(|b| b.usn())
        .map(str::to_string)
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| dev_info.macaddress.eth0.clone());
    let upnp_uuid = discovery::upnp_identity(ipv4).await.map(|(uuid, _, _)| uuid).unwrap_or_default();

    let snapshot = build_snapshot(key, ip, alias, Some(&dev_info), &dev_name, &reading, now_ms());
    Ok(Probe { snapshot, usn, upnp_uuid })
}

/// The stable key for a freshly-probed device: UPnP uuid, then USN, then IP.
fn stable_key(upnp_uuid: &str, usn: &str, ip: &str) -> String {
    if !upnp_uuid.trim().is_empty() {
        upnp_uuid.trim().to_string()
    } else if !usn.trim().is_empty() {
        usn.trim().to_string()
    } else {
        ip.to_string()
    }
}

fn net_mode_str(mode: Option<NetMode>) -> Option<String> {
    mode.map(|m| match m {
        NetMode::Ethernet => "ethernet".to_string(),
        NetMode::Wifi => "wifi".to_string(),
    })
}

/// FR-5: add a device by IP.
///
/// Confirmed by *talking* to it over Luci (`DevInfo` + a DDMS M-SEARCH), never
/// by pattern-matching. Nothing is persisted until that succeeds. Idempotent:
/// re-adding a known device refreshes its IP.
#[tauri::command]
pub async fn add_device(app: AppHandle, ip: String) -> AppResult<DeviceSnapshot> {
    let ip = ip.trim().to_string();
    if ip.parse::<Ipv4Addr>().is_err() {
        return Err(AppError::InvalidInput(format!(
            "“{ip}” is not a valid IPv4 address (for example 192.168.1.42)."
        )));
    }

    // Probe once with a provisional key to learn the identity, then re-stamp the
    // snapshot with the final stable key.
    let provisional = probe(&ip, &ip, None).await?;
    let key = stable_key(&provisional.upnp_uuid, &provisional.usn, &ip);
    let mut snapshot = provisional.snapshot;
    snapshot.uuid = key.clone();

    let entry = SavedDevice {
        uuid: key.clone(),
        usn: provisional.usn,
        ip: ip.clone(),
        alias: None,
        net_mode: net_mode_str(snapshot.net_mode),
        last_seen: snapshot.last_seen,
        pinned_manual: true,
    };
    store::update(&app, move |config| {
        if let Some(existing) = config.devices.iter_mut().find(|d| d.uuid == entry.uuid) {
            if existing.ip != entry.ip {
                log::info!("{} moved from {} to {}", entry.uuid, existing.ip, entry.ip);
            }
            existing.ip = entry.ip.clone();
            existing.usn = entry.usn.clone();
            existing.net_mode = entry.net_mode.clone();
            existing.last_seen = entry.last_seen;
            existing.pinned_manual = true;
            return Ok(());
        }
        config.devices.push(entry);
        Ok(())
    })?;

    let state = app.state::<AppState>();
    state.poller.put(snapshot.clone());
    state.poller.start(&app, key, ip);
    Ok(snapshot)
}

/// FR-8: forget a device. Stops its poll task first.
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

/// FR-7: set a local alias, optionally pushing the name to the device via
/// `DevName(90)` WRITE. The push happens first so a local alias never claims a
/// rename that never reached the hardware.
#[tauri::command]
pub async fn rename_device(
    app: AppHandle,
    uuid: String,
    alias: Option<String>,
    push_to_device: bool,
) -> AppResult<DeviceSnapshot> {
    let device = saved(&app, &uuid)?;
    let alias = alias.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());

    if push_to_device {
        let name = alias.clone().ok_or_else(|| {
            AppError::InvalidInput("Enter a name before pushing it to the device.".into())
        })?;
        let (client, _events) = LuciClient::connect(&device.ip).await?;
        client.request(MessageBox::DevName.id(), MessageType::Write, &name).await?;
    }

    let stored = alias.clone();
    store::update(&app, move |config| match config.devices.iter_mut().find(|d| d.uuid == uuid) {
        Some(d) => {
            d.alias = stored;
            Ok(())
        }
        None => Err(AppError::NotFound("That device is no longer in the list.".into())),
    })?;

    // Reflect the new name immediately; the kicked poll cycle confirms it.
    let state = app.state::<AppState>();
    let snapshot = match state.poller.snapshot(&device.uuid) {
        Some(mut snapshot) => {
            snapshot.alias = alias.clone();
            snapshot.refresh_display_name();
            snapshot
        }
        None => DeviceSnapshot::offline(&device.uuid, &device.ip, alias, device.last_seen),
    };
    state.poller.put(snapshot.clone());
    state.poller.kick(&device.uuid);
    Ok(snapshot)
}

/// FR-6: the persisted list, hydrated with the poller's latest state.
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

/// FR-9: the detail view — a *live* Luci round trip, including raw payloads.
#[tauri::command]
pub async fn get_status(app: AppHandle, uuid: String) -> AppResult<DeviceDetail> {
    let device = saved(&app, &uuid)?;
    let mut probed = probe(&device.ip, &device.uuid, device.alias.clone()).await?;
    // Keep the saved identity even if the probe recomputed one.
    probed.snapshot.uuid = device.uuid.clone();
    app.state::<AppState>().poller.put(probed.snapshot.clone());

    let raw = probed.snapshot.raw.clone();
    Ok(DeviceDetail { snapshot: probed.snapshot, raw })
}

/// Wake a device's poll loop now. Starts a task for a saved device that has none.
#[tauri::command]
pub fn refresh_device(app: AppHandle, uuid: String) -> AppResult<()> {
    let device = saved(&app, &uuid)?;
    let state = app.state::<AppState>();
    if !state.poller.kick(&device.uuid) {
        state.poller.start(&app, device.uuid, device.ip);
    }
    Ok(())
}

/// Best-effort LAN address of this machine — shown in the debug pane, seeds the
/// subnet sweep.
#[tauri::command]
pub fn local_address() -> Option<String> {
    crate::net::local_ipv4().map(|ip| ip.to_string())
}
