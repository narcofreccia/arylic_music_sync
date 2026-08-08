//! Live Luci probe against real hardware (Phase R1 acceptance).
//!
//! Connects to a device over TLS with the embedded client cert, subscribes to
//! async events, then reads DevInfo(92) and VOLUME(64) and prints them.
//!
//! Run: `cargo run --example luci_probe -- 192.168.10.104`
//! (defaults to 192.168.10.104 — "Lofficina-main", wired — when no IP is given).

use music_sync_lib::luci::messagebox::MessageBox;
use music_sync_lib::luci::model::{DevInfo, DdmsBanner};
use music_sync_lib::luci::LuciClient;

#[tokio::main]
async fn main() {
    let ip = std::env::args().nth(1).unwrap_or_else(|| "192.168.10.104".to_string());
    println!("connecting to {ip}:7777 over TLS…");

    let (client, mut events) = match LuciClient::connect(&ip).await {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("connect failed: {e:?}");
            std::process::exit(1);
        }
    };
    println!("connected + REG_ASYNC_EVENTS acked");

    match client.read(MessageBox::DevInfo).await {
        Ok(payload) => {
            println!("\nDevInfo(92) raw:\n{payload}");
            match DevInfo::parse(&payload) {
                Some(info) => println!(
                    "  parsed: fw={} mcu={} eth0={} wlan0={} serial={}",
                    info.versioninfo.devicefwversion,
                    info.versioninfo.mcuversion,
                    info.macaddress.eth0,
                    info.macaddress.wlan0,
                    info.serialnumber.device_serialnumber
                ),
                None => println!("  (could not parse as DevInfo JSON)"),
            }
        }
        Err(e) => eprintln!("DevInfo(92) failed: {e:?}"),
    }

    match client.read(MessageBox::Volume).await {
        Ok(payload) => println!("\nVOLUME(64) = {payload:?}"),
        Err(e) => eprintln!("VOLUME(64) failed: {e:?}"),
    }

    for mb in [MessageBox::MuteUnmute, MessageBox::PlayState, MessageBox::CurrSource, MessageBox::DevName] {
        match client.read(mb).await {
            Ok(p) => println!("{mb:?} = {p:?}"),
            Err(e) => eprintln!("{mb:?} failed: {e:?}"),
        }
    }

    // DDMS banner (topology) via a direct M-SEARCH.
    match music_sync_lib::discovery::ddms_probe(&ip, std::time::Duration::from_secs(3)).await {
        Some(banner) => {
            let b = DdmsBanner::parse(&banner);
            println!("\nDDMS banner: state={:?} netmode={:?} band={:?} model={:?}", b.state(), b.net_mode(), b.wifi_band(), b.model());
        }
        None => println!("\nDDMS banner: no reply"),
    }

    // Drain a couple of async pushes if any arrive quickly.
    println!("\nlistening for async pushes (2s)…");
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(2));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            ev = events.recv() => match ev {
                Some((mb, status, payload)) => println!("  push {mb:?} status={status} payload={payload:?}"),
                None => break,
            }
        }
    }
    println!("done.");
}
