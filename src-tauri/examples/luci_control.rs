//! Live control derivation for Phase R3 — volume, mute and transport payloads.
//!
//! Safe on the idle wired main unit (192.168.10.104). Sets volume to 22 then
//! back to 30, toggles mute and back, reads the now-playing sources, and probes
//! PLAYCNTRL(40) transport payloads while watching PLAY_STATE(51).
//!
//! Run: `cargo run --example luci_control -- 192.168.10.104 [--transport]`
//! `--transport` also exercises PLAYCNTRL verbs (only pass on an idle unit).

use std::time::Duration;

use music_sync_lib::luci::messagebox::{MessageBox, MessageType};
use music_sync_lib::luci::LuciClient;

async fn read(client: &LuciClient, mb: MessageBox) -> String {
    match client.read(mb).await {
        Ok(p) => p,
        Err(e) => format!("<err: {e}>"),
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let ip = args.next().unwrap_or_else(|| "192.168.10.104".to_string());
    let do_transport = args.any(|a| a == "--transport");

    println!("connecting to {ip}:7777…");
    let (client, _events) = LuciClient::connect(&ip).await.expect("connect");
    println!("connected.\n");

    // -- current state --
    println!("== current state ==");
    for mb in [
        MessageBox::Volume,
        MessageBox::MuteUnmute,
        MessageBox::PlayState,
        MessageBox::CurrSource,
        MessageBox::PlayBackSource,
        MessageBox::TrackInfo,
    ] {
        println!("  {mb:?} = {:?}", read(&client, mb).await);
    }

    // -- VOLUME write --
    println!("\n== VOLUME(64) write test ==");
    let before = read(&client, MessageBox::Volume).await;
    println!("  before: {before:?}");
    match client.write(MessageBox::Volume, "22").await {
        Ok(r) => println!("  WRITE \"22\" ok (reply {r:?})"),
        Err(e) => println!("  WRITE \"22\" FAILED: {e}"),
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    println!("  after 22: {:?}", read(&client, MessageBox::Volume).await);
    match client.write(MessageBox::Volume, "30").await {
        Ok(r) => println!("  WRITE \"30\" ok (reply {r:?})"),
        Err(e) => println!("  WRITE \"30\" FAILED: {e}"),
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    println!("  restored: {:?}", read(&client, MessageBox::Volume).await);

    // -- MUTE toggle --
    println!("\n== Mute_Unmute(63) write test ==");
    let m0 = read(&client, MessageBox::MuteUnmute).await;
    println!("  before: {m0:?}");
    match client.write(MessageBox::MuteUnmute, "MUTE").await {
        Ok(r) => println!("  WRITE \"MUTE\" ok (reply {r:?})"),
        Err(e) => println!("  WRITE \"MUTE\" FAILED: {e}"),
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    println!("  after MUTE: {:?}", read(&client, MessageBox::MuteUnmute).await);
    match client.write(MessageBox::MuteUnmute, "UNMUTE").await {
        Ok(r) => println!("  WRITE \"UNMUTE\" ok (reply {r:?})"),
        Err(e) => println!("  WRITE \"UNMUTE\" FAILED: {e}"),
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    println!("  restored: {:?}", read(&client, MessageBox::MuteUnmute).await);

    // -- TRANSPORT --
    if do_transport {
        println!("\n== PLAYCNTRL(40) transport payload probe ==");
        for payload in ["PAUSE", "PLAY", "STOP", "NEXT", "PREV", "PREVIOUS", "0", "1", "2", "3", "4"] {
            let s0 = read(&client, MessageBox::PlayState).await;
            let res = client.request(MessageBox::Playcntrl.id(), MessageType::Write, payload).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
            let s1 = read(&client, MessageBox::PlayState).await;
            match res {
                Ok(r) => println!("  {payload:>9} -> ok reply={r:?}  play_state {s0:?} => {s1:?}"),
                Err(e) => println!("  {payload:>9} -> ERR {e}  play_state {s0:?} => {s1:?}"),
            }
        }
    } else {
        println!("\n(skipping transport; pass --transport on an idle unit)");
    }

    println!("\ndone.");
}
