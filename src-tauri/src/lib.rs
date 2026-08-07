//! MusicSync desktop core.
//!
//! The Rust side owns everything that touches the network: LP10 discovery
//! (mDNS/SSDP + subnet sweep), the typed Linkplay HTTP client, the status
//! poller and the group-integrity guard. Doing the device HTTP from here (not
//! the webview) sidesteps CORS/mixed-content entirely — see brief.md NFR-4.
//!
//! The SvelteKit frontend talks to it exclusively through Tauri commands and
//! events. No commands exist yet: this is the M1 scaffold.

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
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running MusicSync");
}
