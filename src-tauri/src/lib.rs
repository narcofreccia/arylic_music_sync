//! MusicSync desktop core.
//!
//! The Rust side owns everything that touches the network: LP10 discovery
//! (DDMS M-SEARCH + SSDP + subnet sweep), the persistent per-device Luci client,
//! the status poller and (later) the group-integrity guard. Doing the device I/O
//! from here (not the webview) sidesteps CORS/mixed-content entirely, and the
//! Luci channel needs a native TLS stack the browser cannot offer.
//!
//! The SvelteKit frontend talks to it exclusively through Tauri commands and
//! events. R1 rebases the device-control layer onto the LibreWireless **Luci**
//! protocol (`luci`), because the LP10 does not speak the classic Linkplay
//! httpapi (docs/firmware-notes.md). Grouping (R2+) builds on the DDMS state
//! this layer already reads.

pub mod commands;
pub mod discovery;
pub mod error;
pub mod luci;
pub mod net;
pub mod poller;
pub mod spotify;
pub mod state;
pub mod store;
pub mod streaming;
pub mod upnp;

use tauri::Manager;

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().build())
        // FR-27 start-at-login. LaunchAgent on macOS; the plugin picks the
        // platform-appropriate mechanism elsewhere. No launch args are passed.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // The store plugin must already be registered, hence loading here
            // rather than before the builder. A read failure is not fatal: we
            // fall back to defaults, which the frontend reads as "first launch"
            // and answers with the setup wizard.
            let config = store::load(app.handle()).unwrap_or_else(|e| {
                log::error!("failed to load {}: {e}", store::STORE_FILE);
                store::Config::default()
            });
            app.manage(AppState::new(config));
            // Let the streaming engine (S2) emit `stream-state` events to the UI.
            app.state::<AppState>()
                .streaming
                .set_app_handle(app.handle().clone());
            // Let the Spotify manager (S3) emit `spotify-state` events to the UI.
            app.state::<AppState>()
                .spotify
                .set_app_handle(app.handle().clone());
            // FR-6: re-poll the known devices from launch, so the list already
            // shows real online/offline state by the time the user gets past
            // the login screen.
            poller::start_saved(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::auth::auth_state,
            commands::auth::create_profile,
            commands::auth::login,
            commands::auth::logout,
            commands::auth::set_password,
            commands::auth::remove_password,
            commands::auth::set_remember_me,
            commands::devices::add_device,
            commands::devices::remove_device,
            commands::devices::rename_device,
            commands::devices::list_devices,
            commands::devices::get_status,
            commands::devices::refresh_device,
            commands::devices::local_address,
            commands::playback::set_volume,
            commands::playback::set_mute,
            commands::playback::player_cmd,
            commands::scan::scan,
            commands::scan::cancel_scan,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::set_subnet,
            commands::settings::set_poll_profile,
            commands::settings::export_config,
            commands::settings::import_config,
            commands::settings::export_config_file,
            commands::settings::import_config_file,
            commands::streaming::stream_start,
            commands::streaming::stream_stop,
            commands::streaming::stream_set_device_volume,
            commands::streaming::stream_set_device_delay,
            commands::streaming::stream_status,
            commands::spotify::spotify_start,
            commands::spotify::spotify_stop,
            commands::spotify::spotify_status,
            commands::spotify::spotify_play,
            commands::spotify::spotify_pause,
            commands::spotify::spotify_next,
            commands::spotify::spotify_prev,
            commands::spotify::spotify_set_volume,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MusicSync");
}
