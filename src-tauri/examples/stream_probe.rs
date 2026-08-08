//! Sync-proof probe for the RAOP multi-sender (Phase S2).
//!
//! Drives the real [`StreamEngine`] against two (or more) local RAOP receivers —
//! e.g. two `shairport-sync` instances stood up by `scripts/streaming_test_rig.sh`
//! — streaming an identical test tone to all of them off one shared NTP anchor.
//! The rig then cross-correlates the receivers' captured output to measure the
//! inter-receiver sync lag.
//!
//! Run (defaults to 127.0.0.1:5000 and :5010, a 12 s 1 kHz tone):
//!   cargo run --example stream_probe
//!   cargo run --example stream_probe -- 127.0.0.1:5000 127.0.0.1:5010
//!   STREAM_PROBE_SECS=8 STREAM_PROBE_FREQ=440 cargo run --example stream_probe -- 127.0.0.1:5000

use std::time::Duration;

use music_sync_lib::streaming::model::{StreamSource, StreamTarget};
use music_sync_lib::streaming::StreamEngine;

fn parse_target(spec: &str, idx: usize) -> StreamTarget {
    let (ip, port) = spec
        .rsplit_once(':')
        .map(|(i, p)| (i.to_string(), p.parse().unwrap_or(5000)))
        .unwrap_or((spec.to_string(), 5000));
    StreamTarget {
        uuid: None,
        name: format!("FakeLP10-{}", (b'A' + idx as u8) as char),
        ip,
        raop_port: port,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let specs: Vec<String> = if args.is_empty() {
        vec!["127.0.0.1:5000".into(), "127.0.0.1:5010".into()]
    } else {
        args
    };
    let targets: Vec<StreamTarget> = specs
        .iter()
        .enumerate()
        .map(|(i, s)| parse_target(s, i))
        .collect();

    let secs: u64 = std::env::var("STREAM_PROBE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    let freq: u32 = std::env::var("STREAM_PROBE_FREQ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);

    let engine = StreamEngine::default();
    let bin = match engine.resolve_binary() {
        Some(p) => p,
        None => {
            eprintln!(
                "cliraop binary not found. Run scripts/fetch_cliraop.sh (or place \
                 cliraop-<target-triple> under src-tauri/binaries/)."
            );
            std::process::exit(2);
        }
    };
    println!("using cliraop: {}", bin.display());
    println!(
        "targets: {}",
        targets
            .iter()
            .map(|t| format!("{}:{}", t.ip, t.raop_port))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // The tone is a touch longer than the capture window so cross-correlation has
    // steady-state signal on both receivers.
    let source = StreamSource::Tone {
        freq_hz: freq,
        duration_ms: (secs as u32 + 3) * 1000,
    };

    match engine.start(bin, targets, source, None, None) {
        Ok(status) => {
            println!(
                "stream started: anchor_ntp={:?} latency={}f devices={}",
                status.anchor_ntp,
                status.latency_frames,
                status.devices.len()
            );
        }
        Err(e) => {
            eprintln!("stream_start failed: {e}");
            std::process::exit(1);
        }
    }

    println!("streaming for {secs}s (Ctrl-C to stop early)…");
    std::thread::sleep(Duration::from_secs(secs));

    let st = engine.status();
    for d in &st.devices {
        println!(
            "  {} {}:{}  alive={} frames={} vol={:.2} delay={}ms",
            d.name, d.ip, d.raop_port, d.alive, d.frames_written, d.volume, d.delay_ms
        );
    }

    match engine.stop() {
        Ok(_) => println!("stream stopped."),
        Err(e) => eprintln!("stream_stop failed: {e}"),
    }
}
