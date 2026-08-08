//! The Spotify Connect session manager (Phase S3).
//!
//! Owns the whole in-process librespot stack for one advertised endpoint:
//! a [`Session`], a [`Player`] wired to our capturing [`RingSink`], a soft
//! [`Mixer`], and a zeroconf [`Discovery`] advertising **"MusicSync"** as a
//! Spotify Connect *Speaker*. The user selects it in their Spotify app (Premium),
//! which hands us [`Credentials`]; we then bring up a [`Spirc`] (the Connect
//! state machine) and pump its player events into a [`SpotifyState`] mirrored to
//! the frontend via the `spotify-state` Tauri event.
//!
//! librespot is async; since [`AppState`](crate::state::AppState) is a synchronous
//! container, the manager owns a dedicated multi-thread Tokio runtime and runs the
//! discovery/event loops as tasks on it. All librespot construction happens *inside*
//! that runtime (mdns/dealer tasks need a reactor). Transport control
//! (play/pause/next/prev/volume) is proxied to the live [`Spirc`] handle.
//!
//! Decoded PCM does **not** flow through here to the network directly: the
//! `RingSink` pushes it into the shared [`PcmFanout`], and the S2 streaming engine
//! tees it from there to the `cliraop` children. The manager thus never touches
//! RAOP — it only produces PCM + metadata.

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use tauri::{AppHandle, Emitter};

use librespot_connect::{ConnectConfig, Spirc};
use librespot_core::config::DeviceType;
use librespot_core::{Session, SessionConfig};
use librespot_discovery::{Credentials, Discovery};
use librespot_playback::config::PlayerConfig;
use librespot_playback::mixer::{Mixer, MixerConfig};
use librespot_playback::mixer::softmixer::SoftMixer;
use librespot_playback::player::{Player, PlayerEvent};

use crate::spotify::sink::RingSink;
use crate::spotify::state::{PlayState, SpotifyState};
use crate::streaming::live::PcmFanout;

/// The zeroconf-advertised Connect device name the user picks in Spotify.
pub const DEVICE_NAME: &str = "MusicSync";

/// A running librespot session: the runtime, the shutdown signal, and the live
/// Spirc handle used for transport control.
struct Running {
    runtime: tokio::runtime::Runtime,
    /// Flipped true to ask the loops to exit; observed via `shutdown_tx`.
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Populated by the discovery loop once a Spotify client connects.
    spirc: Arc<Mutex<Option<Spirc>>>,
}

/// The Spotify manager held in `AppState`. `Send + Sync`; all mutation goes
/// through the inner mutexes.
pub struct SpotifyManager {
    inner: Mutex<Option<Running>>,
    /// The live PCM tee, shared with the streaming engine. Stable across
    /// start/stop so a `stream_start { source: Spotify }` can always reach it.
    fanout: Arc<PcmFanout>,
    /// Last known state, mirrored to `spotify-state` and returned by status().
    state: Arc<Mutex<SpotifyState>>,
    app: Mutex<Option<AppHandle>>,
}

impl Default for SpotifyManager {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
            fanout: Arc::new(PcmFanout::new()),
            state: Arc::new(Mutex::new(SpotifyState::default())),
            app: Mutex::new(None),
        }
    }
}

impl SpotifyManager {
    /// Store the Tauri handle so state changes reach the frontend as
    /// `spotify-state` events. Optional — the example runs without one.
    pub fn set_app_handle(&self, handle: AppHandle) {
        if let Ok(mut a) = self.app.lock() {
            *a = Some(handle);
        }
    }

    /// The live PCM fan-out the streaming engine subscribes to.
    pub fn fanout(&self) -> Arc<PcmFanout> {
        self.fanout.clone()
    }

    /// Whether the endpoint is currently advertising.
    pub fn is_running(&self) -> bool {
        self.inner.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Snapshot of the current state (idle when not running).
    pub fn status(&self) -> SpotifyState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Start advertising the "MusicSync" Spotify Connect endpoint.
    ///
    /// Spins up a dedicated runtime, builds the librespot stack inside it, and
    /// launches the zeroconf advertisement + event loops. Returns as soon as the
    /// endpoint is up; the user then picks "MusicSync" in their Spotify app and
    /// authorizes (no stored password — Connect discovery mode). Idempotent guard:
    /// errors if already running.
    pub fn start(&self) -> Result<SpotifyState, String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Err("Spotify capture already running; stop it first".into());
        }

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("musicsync-spotify")
            .build()
            .map_err(|e| format!("failed to build Spotify runtime: {e}"))?;

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let spirc: Arc<Mutex<Option<Spirc>>> = Arc::new(Mutex::new(None));

        // Reset and mark the mirrored state as running/advertising.
        {
            let mut st = self.state.lock().map_err(|e| e.to_string())?;
            *st = SpotifyState {
                running: true,
                device_name: DEVICE_NAME.to_string(),
                ..SpotifyState::default()
            };
        }

        let ctx = SessionContext {
            fanout: self.fanout.clone(),
            state: self.state.clone(),
            app: self.app.lock().ok().and_then(|a| a.clone()),
            spirc: spirc.clone(),
            shutdown_rx,
        };
        // All librespot construction lives inside the runtime (needs a reactor).
        runtime.spawn(async move {
            if let Err(e) = run_session(ctx).await {
                log::error!("spotify session ended with error: {e}");
            }
        });

        *guard = Some(Running {
            runtime,
            shutdown_tx,
            spirc,
        });
        drop(guard);

        let status = self.status();
        self.emit(&status);
        log::info!("spotify: advertising Connect endpoint \"{DEVICE_NAME}\"");
        Ok(status)
    }

    /// Stop advertising and tear the session down. Idempotent.
    pub fn stop(&self) -> Result<SpotifyState, String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        if let Some(running) = guard.take() {
            // Ask the loops to exit, shut the Spirc down, then drop the runtime
            // in the background (non-blocking — we may be on a Tauri worker).
            let _ = running.shutdown_tx.send(true);
            if let Ok(mut s) = running.spirc.lock() {
                if let Some(spirc) = s.take() {
                    let _ = spirc.shutdown();
                }
            }
            running.runtime.shutdown_background();
        }
        drop(guard);
        // Drop any engine subscribers still holding the tee.
        self.fanout.clear();

        if let Ok(mut st) = self.state.lock() {
            *st = SpotifyState::default();
        }
        let status = self.status();
        self.emit(&status);
        log::info!("spotify: capture stopped");
        Ok(status)
    }

    /// Resume playback on the connected Spotify session.
    pub fn play(&self) -> Result<(), String> {
        self.with_spirc(|s| s.play())
    }
    /// Pause playback.
    pub fn pause(&self) -> Result<(), String> {
        self.with_spirc(|s| s.pause())
    }
    /// Skip to the next track.
    pub fn next(&self) -> Result<(), String> {
        self.with_spirc(|s| s.next())
    }
    /// Skip to the previous track.
    pub fn prev(&self) -> Result<(), String> {
        self.with_spirc(|s| s.prev())
    }

    /// Set Connect volume from a `0.0..=1.0` level (mapped to Spotify's 16-bit
    /// scale), matching the streaming engine's volume convention.
    pub fn set_volume(&self, level: f32) -> Result<(), String> {
        let vol = (level.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16;
        self.with_spirc(|s| s.set_volume(vol))
    }

    /// Run `f` against the live Spirc handle, mapping its error to a String.
    fn with_spirc<F>(&self, f: F) -> Result<(), String>
    where
        F: FnOnce(&Spirc) -> Result<(), librespot_core::Error>,
    {
        let guard = self.inner.lock().map_err(|e| e.to_string())?;
        let running = guard.as_ref().ok_or("Spotify capture is not running")?;
        let spirc_guard = running.spirc.lock().map_err(|e| e.to_string())?;
        let spirc = spirc_guard
            .as_ref()
            .ok_or("no Spotify client connected yet (pick MusicSync in Spotify)")?;
        f(spirc).map_err(|e| e.to_string())
    }

    fn emit(&self, state: &SpotifyState) {
        if let Ok(a) = self.app.lock() {
            if let Some(handle) = a.as_ref() {
                let _ = handle.emit("spotify-state", state);
            }
        }
    }
}

/// Everything the async session task needs, moved into the runtime.
struct SessionContext {
    fanout: Arc<PcmFanout>,
    state: Arc<Mutex<SpotifyState>>,
    app: Option<AppHandle>,
    spirc: Arc<Mutex<Option<Spirc>>>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

/// Build the librespot stack and run the discovery + player-event loops until
/// shutdown. This is the async heart of the manager, run on its own runtime.
async fn run_session(ctx: SessionContext) -> Result<(), String> {
    let SessionContext {
        fanout,
        state,
        app,
        spirc,
        mut shutdown_rx,
    } = ctx;

    // One SessionConfig gives us a stable device_id + client_id shared between the
    // session and the zeroconf advertisement.
    let session_config = SessionConfig::default();
    let device_id = session_config.device_id.clone();
    let client_id = session_config.client_id.clone();

    let session = Session::new(session_config, None);

    // Soft mixer (we never touch a real device); the player reads its volume.
    let mixer: Arc<dyn Mixer> =
        Arc::new(SoftMixer::open(MixerConfig::default()).map_err(|e| e.to_string())?);

    // Build the Player with OUR capturing sink instead of any audio device.
    let player_config = PlayerConfig::default();
    let sink_fanout = fanout.clone();
    let player = Player::new(
        player_config,
        session.clone(),
        mixer.get_soft_volume(),
        move || Box::new(RingSink::new(sink_fanout)),
    );

    // Pump player events (metadata, play/pause, volume) into the mirrored state.
    let events_state = state.clone();
    let events_app = app.clone();
    let mut events_shutdown = shutdown_rx.clone();
    let event_channel = player.get_player_event_channel();
    tokio::spawn(async move {
        run_event_loop(event_channel, events_state, events_app, &mut events_shutdown).await;
    });

    // Advertise "MusicSync" over zeroconf (libmdns). The Stream yields Credentials
    // each time a Spotify client selects and authorizes this endpoint.
    let mut discovery = Discovery::builder(device_id, client_id)
        .name(DEVICE_NAME)
        .device_type(DeviceType::Speaker)
        .launch()
        .map_err(|e| format!("failed to launch zeroconf discovery: {e}"))?;

    let connect_config = ConnectConfig {
        name: DEVICE_NAME.to_string(),
        device_type: DeviceType::Speaker,
        ..ConnectConfig::default()
    };

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            creds = discovery.next() => {
                match creds {
                    Some(credentials) => {
                        handle_credentials(
                            &connect_config,
                            &session,
                            credentials,
                            &player,
                            &mixer,
                            &spirc,
                        )
                        .await;
                    }
                    None => break, // discovery stream ended
                }
            }
        }
    }

    // Cleanup: shut down any live Spirc and the discovery advertisement.
    if let Ok(mut s) = spirc.lock() {
        if let Some(existing) = s.take() {
            let _ = existing.shutdown();
        }
    }
    discovery.shutdown().await;
    log::info!("spotify: session loop exited");
    Ok(())
}

/// A Spotify client selected "MusicSync": bring up a fresh Spirc bound to these
/// credentials (replacing any previous one) and spawn its driving future.
async fn handle_credentials(
    connect_config: &ConnectConfig,
    session: &Session,
    credentials: Credentials,
    player: &Arc<Player>,
    mixer: &Arc<dyn Mixer>,
    spirc_slot: &Arc<Mutex<Option<Spirc>>>,
) {
    // Replace any previous Spirc (a re-selection from another client).
    if let Ok(mut s) = spirc_slot.lock() {
        if let Some(existing) = s.take() {
            let _ = existing.shutdown();
        }
    }

    match Spirc::new(
        connect_config.clone(),
        session.clone(),
        credentials,
        player.clone(),
        mixer.clone(),
    )
    .await
    {
        Ok((new_spirc, spirc_task)) => {
            if let Ok(mut s) = spirc_slot.lock() {
                *s = Some(new_spirc);
            }
            tokio::spawn(spirc_task);
            log::info!("spotify: Connect session established");
        }
        Err(e) => log::error!("spotify: failed to establish Connect session: {e}"),
    }
}

/// Translate librespot `PlayerEvent`s into `SpotifyState` mutations + emits.
async fn run_event_loop(
    mut channel: librespot_playback::player::PlayerEventChannel,
    state: Arc<Mutex<SpotifyState>>,
    app: Option<AppHandle>,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            event = channel.recv() => {
                match event {
                    Some(ev) => {
                        if apply_event(&state, ev) {
                            emit_state(&app, &state);
                        }
                    }
                    None => break,
                }
            }
        }
    }
}

/// Apply one event to the mirrored state. Returns whether the state changed
/// meaningfully enough to warrant an emit.
fn apply_event(state: &Arc<Mutex<SpotifyState>>, event: PlayerEvent) -> bool {
    use crate::spotify::meta::TrackMeta;
    let mut st = match state.lock() {
        Ok(s) => s,
        Err(_) => return false,
    };
    match event {
        PlayerEvent::SessionConnected { .. } => {
            st.connected = true;
            true
        }
        PlayerEvent::SessionDisconnected { .. } => {
            st.connected = false;
            st.play_state = PlayState::Stopped;
            true
        }
        PlayerEvent::TrackChanged { audio_item } => {
            st.track = Some(TrackMeta::from_audio_item(&audio_item));
            true
        }
        PlayerEvent::Playing { position_ms, .. } => {
            st.play_state = PlayState::Playing;
            st.position_ms = position_ms;
            true
        }
        PlayerEvent::Paused { position_ms, .. } => {
            st.play_state = PlayState::Paused;
            st.position_ms = position_ms;
            true
        }
        PlayerEvent::Stopped { .. } => {
            st.play_state = PlayState::Stopped;
            true
        }
        PlayerEvent::VolumeChanged { volume } => {
            st.volume = volume;
            true
        }
        _ => false,
    }
}

fn emit_state(app: &Option<AppHandle>, state: &Arc<Mutex<SpotifyState>>) {
    if let Some(handle) = app {
        if let Ok(st) = state.lock() {
            let _ = handle.emit("spotify-state", &*st);
        }
    }
}
