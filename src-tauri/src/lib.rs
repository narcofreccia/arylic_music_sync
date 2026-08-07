//! MusicSync desktop core.
//!
//! The Rust side owns everything that touches the network: LP10 discovery
//! (mDNS/SSDP + subnet sweep), the typed Linkplay HTTP client, the status
//! poller and the group-integrity guard. Doing the device HTTP from here (not
//! the webview) sidesteps CORS/mixed-content entirely — see brief.md NFR-4.
//!
//! The SvelteKit frontend talks to it exclusively through Tauri commands and
//! events. M1 ships the local-profile auth surface (`commands::auth`); the
//! device layers land in M2+.

pub mod commands;
pub mod error;
pub mod state;
pub mod store;

use tauri::Manager;

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().build())
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running MusicSync");
}
