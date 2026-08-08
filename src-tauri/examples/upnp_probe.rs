//! Read-only UPnP AVTransport probe (Phase R3). Reads GetPositionInfo — safe,
//! it does not change playback. Run: `cargo run --example upnp_probe -- <ip>`

#[tokio::main]
async fn main() {
    let ip = std::env::args().nth(1).unwrap_or_else(|| "192.168.10.148".to_string());
    println!("GetPositionInfo on {ip}:49494…");
    match music_sync_lib::upnp::now_playing(&ip).await {
        Ok(np) => println!(
            "  title={:?}\n  artist={:?}\n  album={:?}\n  duration_ms={:?}\n  position_ms={:?}\n  empty={}",
            np.title, np.artist, np.album, np.duration_ms, np.position_ms, np.is_empty()
        ),
        Err(e) => println!("  ERR: {e}"),
    }
}
