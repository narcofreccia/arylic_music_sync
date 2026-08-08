//! Prove Spotify capture works end to end — no LP10 hardware required (Phase S3).
//!
//! Starts the in-process librespot stack, which advertises a Spotify **Connect**
//! endpoint named **"MusicSync"** over zeroconf. You then pick "MusicSync" in your
//! Spotify app (Premium) and press play; this example captures the decoded PCM off
//! the streaming engine's live fan-out and writes ~15 s of it to a WAV file. If the
//! WAV contains real audio, decode + capture + format conversion all work.
//!
//! ## Manual test steps (at home)
//!   1. `cargo run --example spotify_capture`
//!      (optionally: `SPOTIFY_CAPTURE_SECS=20 SPOTIFY_CAPTURE_OUT=/tmp/cap.wav …`)
//!   2. Open Spotify on any device on the same LAN (phone/desktop, **Premium**).
//!   3. Tap the "Connect to a device" icon → pick **MusicSync**.
//!   4. Press play. Within a second or two the example starts capturing.
//!   5. It writes the WAV and exits. Play it back:  `afplay <out.wav>` (macOS).
//!
//! ## Piping the live capture through the S2 engine to a shairport-sync receiver
//! (the full sender path, still no LP10). In one terminal, stand up a fake speaker:
//!
//!   shairport-sync -a "FakeLP10-A" -o pipe -- /tmp/fakeA.pcm --port 5000
//!
//! then, instead of writing a WAV, call the engine's live path with that target:
//!
//!   let engine = StreamEngine::default();
//!   let bin = engine.resolve_binary().unwrap();
//!   let target = StreamTarget { uuid: None, name: "FakeLP10-A".into(),
//!                               ip: "127.0.0.1".into(), raop_port: 5000 };
//!   engine.start_live(bin, vec![target], manager.fanout(), None, None)?;
//!
//! The engine tees the same librespot PCM to every `cliraop` child off one shared
//! NTP anchor — the S2 proven multi-receiver sync path — now fed live from Spotify.
//! (This example keeps the WAV path so it needs no `cliraop`/shairport-sync.)
//!
//! NOTE: this example cannot be run by CI or an agent — it needs a real Spotify
//! Premium login performed interactively from the Spotify app. It exists to give a
//! human an exact, repeatable capture proof.

use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use music_sync_lib::spotify::SpotifyManager;
use music_sync_lib::streaming::live::DEFAULT_CAPACITY;
use music_sync_lib::streaming::model::{BYTES_PER_FRAME, SAMPLE_RATE};
use music_sync_lib::streaming::wav::write_wav;

fn main() {
    let secs: u64 = std::env::var("SPOTIFY_CAPTURE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15);
    let out = std::env::var("SPOTIFY_CAPTURE_OUT")
        .unwrap_or_else(|_| std::env::temp_dir().join("musicsync-capture.wav").display().to_string());
    // How long to wait for the user to pick MusicSync + press play before giving up.
    let wait_secs: u64 = std::env::var("SPOTIFY_CAPTURE_WAIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);

    let target_bytes = secs as usize * SAMPLE_RATE as usize * BYTES_PER_FRAME;

    let manager = SpotifyManager::default();
    let fanout = manager.fanout();
    let rx = fanout.subscribe(DEFAULT_CAPACITY);

    if let Err(e) = manager.start() {
        eprintln!("failed to start Spotify capture: {e}");
        std::process::exit(1);
    }

    println!("─────────────────────────────────────────────────────────────");
    println!(" MusicSync Spotify capture proof (Phase S3)");
    println!("─────────────────────────────────────────────────────────────");
    println!(" 1. Open Spotify (Premium) on any device on this LAN.");
    println!(" 2. Tap 'Connect to a device' and pick  \x1b[1mMusicSync\x1b[0m.");
    println!(" 3. Press play. Capturing {secs}s → {out}");
    println!("    (waiting up to {wait_secs}s for playback to start…)");
    println!("─────────────────────────────────────────────────────────────");

    let mut pcm: Vec<u8> = Vec::with_capacity(target_bytes + (1 << 16));
    let deadline = Instant::now() + Duration::from_secs(wait_secs);
    let mut started = false;
    let mut last_report = Instant::now();

    while pcm.len() < target_bytes {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(packet) => {
                if !started {
                    started = true;
                    println!("▶ capturing… (audio detected)");
                }
                pcm.extend_from_slice(&packet);
                if last_report.elapsed() >= Duration::from_secs(1) {
                    let secs_got = pcm.len() as f64 / (SAMPLE_RATE as f64 * BYTES_PER_FRAME as f64);
                    println!("  captured {:.1}s / {}s", secs_got, secs);
                    last_report = Instant::now();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let st = manager.status();
                if !started && Instant::now() >= deadline {
                    eprintln!(
                        "\nTimed out waiting for playback (connected={}, running={}).\n\
                         Did you pick MusicSync in Spotify and press play? Premium is required.",
                        st.connected, st.running
                    );
                    let _ = manager.stop();
                    std::process::exit(2);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                eprintln!("capture channel closed unexpectedly");
                break;
            }
        }
    }

    // Trim to a whole number of frames just in case and write the WAV.
    let keep = pcm.len() - (pcm.len() % BYTES_PER_FRAME);
    pcm.truncate(keep);
    match write_wav(std::path::Path::new(&out), &pcm) {
        Ok(()) => {
            let dur = pcm.len() as f64 / (SAMPLE_RATE as f64 * BYTES_PER_FRAME as f64);
            println!("\n✓ wrote {:.1}s of captured audio → {out}", dur);
            if let Some(track) = manager.status().track {
                println!("  now playing: {} — {}", track.artist, track.title);
            }
            println!("  play it back e.g.:  afplay {out}");
        }
        Err(e) => eprintln!("failed to write WAV: {e}"),
    }

    let _ = manager.stop();
}
