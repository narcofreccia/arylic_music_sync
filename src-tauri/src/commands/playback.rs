//! Per-device playback control (Phase R3) — volume, mute and transport.
//!
//! These are the *user-initiated* mutations; the poller reflects the resulting
//! state a cycle later (each command kicks the device's loop so that happens
//! promptly). A command returns `()` and lets the poller correct the UI, which
//! keeps the optimistic-with-rollback dance on the frontend simple.
//!
//! Channel choice, derived live on real hardware (docs/firmware-notes.md):
//! * **Volume / mute → Luci.** `VOLUME(64)` / `Mute_Unmute(63)` WRITEs apply, but
//!   the firmware sends no reply frame for them, so they go out fire-and-forget
//!   ([`LuciClient::write_oneway`]) — waiting would always time out.
//! * **Transport → UPnP AVTransport, Luci `PLAYCNTRL(40)` fallback.** Luci
//!   `PLAYCNTRL` *acks* every payload without changing `PLAY_STATE` on this
//!   firmware (the same "accepted, no effect" signature as the dead DDMS group
//!   verbs, §G), so the reliable standard AVTransport SOAP path is preferred and
//!   `PLAYCNTRL` is only tried if the UPnP endpoint is unreachable.

use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};
use crate::luci::messagebox::MessageBox;
use crate::luci::LuciClient;
use crate::state::AppState;
use crate::store::{self, SavedDevice};
use crate::upnp::{self, Transport};

/// Look up a saved device, or fail with a stale-reference error.
fn saved(app: &AppHandle, uuid: &str) -> AppResult<SavedDevice> {
    store::get(app)
        .devices
        .into_iter()
        .find(|d| d.uuid == uuid)
        .ok_or_else(|| AppError::NotFound("That device is no longer in the list.".into()))
}

/// Wake the device's poll loop so the confirmed state lands without waiting out
/// the interval.
fn kick(app: &AppHandle, uuid: &str) {
    app.state::<AppState>().poller.kick(uuid);
}

/// Set absolute volume (0..=100). Clamped, then written over Luci `VOLUME(64)`.
#[tauri::command]
pub async fn set_volume(app: AppHandle, uuid: String, vol: u8) -> AppResult<()> {
    let device = saved(&app, &uuid)?;
    let vol = vol.min(100);
    let (client, _events) = LuciClient::connect(&device.ip).await?;
    client.write_oneway(MessageBox::Volume, &vol.to_string()).await?;
    kick(&app, &uuid);
    Ok(())
}

/// Mute or unmute over Luci `Mute_Unmute(63)`. Payload `"MUTE"` / `"UNMUTE"` —
/// both verified live to take effect.
#[tauri::command]
pub async fn set_mute(app: AppHandle, uuid: String, mute: bool) -> AppResult<()> {
    let device = saved(&app, &uuid)?;
    let payload = if mute { "MUTE" } else { "UNMUTE" };
    let (client, _events) = LuciClient::connect(&device.ip).await?;
    client.write_oneway(MessageBox::MuteUnmute, payload).await?;
    kick(&app, &uuid);
    Ok(())
}

/// Transport: `play` | `pause` | `next` | `prev` | `stop`.
///
/// Prefers UPnP AVTransport (the proven per-device transport path); if the UPnP
/// endpoint can't be reached, falls back to Luci `PLAYCNTRL(40)` with the verb as
/// an uppercase ASCII payload.
#[tauri::command]
pub async fn player_cmd(app: AppHandle, uuid: String, cmd: String) -> AppResult<()> {
    let device = saved(&app, &uuid)?;
    let transport = Transport::parse(&cmd)
        .ok_or_else(|| AppError::InvalidInput(format!("Unknown transport command “{cmd}”.")))?;

    match upnp::transport(&device.ip, transport).await {
        Ok(()) => {}
        Err(upnp_err) => {
            log::debug!("{}: UPnP transport failed ({upnp_err}); trying Luci PLAYCNTRL", device.ip);
            let (client, _events) = LuciClient::connect(&device.ip).await?;
            client.write(MessageBox::Playcntrl, playcntrl_verb(transport)).await?;
        }
    }
    kick(&app, &uuid);
    Ok(())
}

/// The uppercase ASCII verb Luci `PLAYCNTRL(40)` takes for a transport action.
fn playcntrl_verb(t: Transport) -> &'static str {
    match t {
        Transport::Play => "PLAY",
        Transport::Pause => "PAUSE",
        Transport::Next => "NEXT",
        Transport::Previous => "PREV",
        Transport::Stop => "STOP",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playcntrl_verbs_cover_every_transport() {
        assert_eq!(playcntrl_verb(Transport::Play), "PLAY");
        assert_eq!(playcntrl_verb(Transport::Pause), "PAUSE");
        assert_eq!(playcntrl_verb(Transport::Next), "NEXT");
        assert_eq!(playcntrl_verb(Transport::Previous), "PREV");
        assert_eq!(playcntrl_verb(Transport::Stop), "STOP");
    }
}
