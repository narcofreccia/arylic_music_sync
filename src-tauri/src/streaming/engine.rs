//! The stream orchestrator (Phase S2).
//!
//! Owns the whole sender pipeline for one active group: capture the shared master
//! NTP clock **once**, spawn one `cliraop` child per receiver anchored to it with
//! matched latency/wait, then fan a single PCM source out to every child — teeing
//! *identical* frames so the receivers stay locked speaker-to-speaker (design doc
//! §1). Per-device software **volume** and **delay** are applied in the PCM domain
//! by each child's writer thread just before its stdin (design doc §4), which
//! sidesteps `cliraop`'s "volume is initial-only" CLI limitation entirely.
//!
//! Sync is *not* enforced by the feed rate — `cliraop` self-paces off the shared
//! NTP, and the OS pipe back-pressures each writer independently — it is enforced
//! by the shared anchor + matched `-l`/`-w`. A slow/failed receiver stalls or kills
//! only its own child, never the group.

use std::path::PathBuf;
use std::process::ChildStdin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use super::live::{PcmFanout, PcmPacket, DEFAULT_CAPACITY};
use super::model::{
    ms_to_frames, DeviceStatus, StreamSource, StreamStatus, StreamTarget, DEFAULT_LATENCY_FRAMES,
    DEFAULT_WAIT_MS,
};
use super::sidecar::{capture_ntp_anchor, resolve_cliraop, RaopChild};
use super::sync::{scale_s16le, silence_frames, FrameChunks};
use super::wav::load_source;

/// The PCM a device writer pulls from: a whole static buffer (S2 file/tone) or a
/// live per-device receiver fed by the [`PcmFanout`] (S3 Spotify capture).
enum ChunkFeed {
    /// The shared static buffer; the writer iterates it once and exits at EOF.
    Static(Arc<Vec<u8>>),
    /// A live tee receiver; the writer pumps packets until stopped or disconnected.
    Live(Receiver<PcmPacket>),
}

/// Frames per stdin write. 4096 frames ≈ 93 ms of audio (16 KB); big enough to
/// keep syscall overhead low, small enough that a live volume change lands
/// promptly and the OS pipe still back-pressures for pacing.
const CHUNK_FRAMES: usize = 4096;

/// Live, per-receiver control shared between the command layer and that device's
/// writer thread. Volume is read every chunk (so it applies live); delay is read
/// once when the writer starts (it shapes the head of the stream).
pub struct DeviceControl {
    pub ip: String,
    pub name: String,
    pub raop_port: u16,
    /// Persistence key for this target's delay (device UUID / manual-id / IP).
    pub key: String,
    /// Gain × 1000, so we can carry it in an atomic. `1000` = unity.
    gain_milli: AtomicU32,
    /// Per-device delay in ms (applied as silent lead-in frames).
    delay_ms: AtomicU32,
    /// Frames of real audio pushed to stdin so far (excludes delay silence).
    frames_written: AtomicU64,
}

impl DeviceControl {
    fn new(target: &StreamTarget) -> Self {
        Self {
            ip: target.ip.clone(),
            name: target.name.clone(),
            raop_port: target.raop_port,
            key: target.delay_key(),
            gain_milli: AtomicU32::new(1000),
            // Seed the delay from the target's persisted value (Feature 2). A
            // default 0 keeps the seamless-sync path untouched.
            delay_ms: AtomicU32::new(target.delay_ms),
            frames_written: AtomicU64::new(0),
        }
    }
    fn gain(&self) -> f32 {
        self.gain_milli.load(Ordering::Relaxed) as f32 / 1000.0
    }
    fn set_gain(&self, gain: f32) {
        let clamped = (gain.clamp(0.0, 1.0) * 1000.0).round() as u32;
        self.gain_milli.store(clamped, Ordering::Relaxed);
    }
    fn delay_ms(&self) -> u32 {
        self.delay_ms.load(Ordering::Relaxed)
    }
    fn set_delay_ms(&self, ms: u32) {
        self.delay_ms.store(ms, Ordering::Relaxed);
    }
    fn add_frames(&self, n: u64) {
        self.frames_written.fetch_add(n, Ordering::Relaxed);
    }
    fn frames(&self) -> u64 {
        self.frames_written.load(Ordering::Relaxed)
    }
}

/// One active streaming session.
struct ActiveStream {
    source_label: String,
    anchor_ntp: String,
    anchor_file: PathBuf,
    latency_frames: u32,
    controls: Vec<Arc<DeviceControl>>,
    /// Kept alive so children are killed when the session ends (via Drop).
    children: Vec<RaopChild>,
    stop: Arc<AtomicBool>,
    writers: Vec<JoinHandle<()>>,
}

impl ActiveStream {
    fn status(&mut self) -> StreamStatus {
        let mut alive: Vec<bool> = Vec::with_capacity(self.children.len());
        for c in self.children.iter_mut() {
            alive.push(c.is_alive());
        }
        let devices = self
            .controls
            .iter()
            .enumerate()
            .map(|(i, ctl)| DeviceStatus {
                ip: ctl.ip.clone(),
                name: ctl.name.clone(),
                raop_port: ctl.raop_port,
                key: ctl.key.clone(),
                volume: ctl.gain(),
                delay_ms: ctl.delay_ms(),
                alive: alive.get(i).copied().unwrap_or(false),
                frames_written: ctl.frames(),
            })
            .collect();
        StreamStatus {
            active: true,
            source: Some(self.source_label.clone()),
            anchor_ntp: Some(self.anchor_ntp.clone()),
            latency_frames: self.latency_frames,
            devices,
        }
    }

    /// Signal writers to stop, kill children, join, and remove the anchor file.
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        for c in self.children.iter_mut() {
            c.kill();
        }
        for w in self.writers.drain(..) {
            let _ = w.join();
        }
        let _ = std::fs::remove_file(&self.anchor_file);
    }
}

impl Drop for ActiveStream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The engine, managed in `AppState`. `Send + Sync`; all mutation goes through
/// the inner mutex.
pub struct StreamEngine {
    inner: Mutex<Option<ActiveStream>>,
    app: Mutex<Option<AppHandle>>,
}

impl Default for StreamEngine {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
            app: Mutex::new(None),
        }
    }
}

impl StreamEngine {
    /// Store the Tauri handle so status changes reach the frontend as
    /// `stream-state` events. Optional — the example runs without one.
    pub fn set_app_handle(&self, handle: AppHandle) {
        if let Ok(mut a) = self.app.lock() {
            *a = Some(handle);
        }
    }

    /// Resolve the bundled `cliraop` binary using the stored `AppHandle`'s
    /// resource/exe dirs when present, else the dev `binaries/` fallback.
    pub fn resolve_binary(&self) -> Option<PathBuf> {
        use tauri::Manager;
        let (resource_dir, exe_dir) = {
            let guard = self.app.lock().ok();
            match guard.as_ref().and_then(|g| g.as_ref()) {
                Some(handle) => (
                    handle.path().resource_dir().ok(),
                    std::env::current_exe()
                        .ok()
                        .and_then(|e| e.parent().map(|p| p.to_path_buf())),
                ),
                None => (None, None),
            }
        };
        resolve_cliraop(resource_dir.as_deref(), exe_dir.as_deref())
    }

    /// Whether a session is currently active.
    pub fn is_active(&self) -> bool {
        self.inner.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Start streaming `source` to every `target`, driven by `bin` (`cliraop`).
    ///
    /// Steps: reject if already active → load the PCM source → capture the shared
    /// NTP anchor once → spawn one child per target with the shared anchor +
    /// matched latency/wait → launch one writer thread per child. Returns the
    /// resulting [`StreamStatus`].
    ///
    /// This is the S2 *static* path (WAV/RAW/tone). The live Spotify capture uses
    /// [`start_live`](Self::start_live), which shares the same child-spawn core.
    pub fn start(
        &self,
        bin: PathBuf,
        targets: Vec<StreamTarget>,
        source: StreamSource,
        latency_frames: Option<u32>,
        wait_ms: Option<u32>,
    ) -> Result<StreamStatus, String> {
        // Load the whole PCM source once; every child is teed the same Arc.
        let pcm = Arc::new(load_source(&source)?);
        if pcm.is_empty() {
            return Err("PCM source is empty".into());
        }
        self.start_internal(bin, targets, source.label(), latency_frames, wait_ms, || {
            ChunkFeed::Static(pcm.clone())
        })
    }

    /// Start streaming the **live** librespot capture (Phase S3) to every `target`.
    ///
    /// Identical to [`start`](Self::start) — shared NTP anchor, matched latency,
    /// one child per receiver, per-device software volume/delay — except the PCM
    /// is teed in real time from `fanout` (fed by the Spotify [`RingSink`]) instead
    /// of a preloaded buffer. Each child gets its own fan-out subscription, so the
    /// decode thread never blocks and a stalled receiver drops only its own frames.
    pub fn start_live(
        &self,
        bin: PathBuf,
        targets: Vec<StreamTarget>,
        fanout: Arc<PcmFanout>,
        latency_frames: Option<u32>,
        wait_ms: Option<u32>,
    ) -> Result<StreamStatus, String> {
        // Drop any stale subscribers from a previous session before we re-subscribe.
        fanout.clear();
        self.start_internal(bin, targets, "spotify".to_string(), latency_frames, wait_ms, || {
            ChunkFeed::Live(fanout.subscribe(DEFAULT_CAPACITY))
        })
    }

    /// Shared start core: capture the anchor once, spawn one child per target, and
    /// launch a writer per child whose PCM feed comes from `make_feed` (a static
    /// buffer for S2, a live fan-out subscription for S3). `make_feed` is called
    /// once per target, in order.
    fn start_internal(
        &self,
        bin: PathBuf,
        targets: Vec<StreamTarget>,
        source_label: String,
        latency_frames: Option<u32>,
        wait_ms: Option<u32>,
        mut make_feed: impl FnMut() -> ChunkFeed,
    ) -> Result<StreamStatus, String> {
        if targets.is_empty() {
            return Err("no stream targets given".into());
        }
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Err("a stream is already active; stop it first".into());
        }

        let latency_frames = latency_frames.unwrap_or(DEFAULT_LATENCY_FRAMES);
        let wait_ms = wait_ms.unwrap_or(DEFAULT_WAIT_MS);

        // Capture the shared master clock ONCE.
        let anchor_file = std::env::temp_dir().join(format!(
            "musicsync-anchor-{}.ntp",
            std::process::id()
        ));
        let anchor_ntp = capture_ntp_anchor(&bin, &anchor_file)?;
        log::info!(
            "stream: captured NTP anchor {anchor_ntp} for {} target(s), latency={latency_frames}f wait={wait_ms}ms source={source_label}",
            targets.len()
        );

        let stop = Arc::new(AtomicBool::new(false));
        let mut controls: Vec<Arc<DeviceControl>> = Vec::with_capacity(targets.len());
        let mut children: Vec<RaopChild> = Vec::with_capacity(targets.len());
        let mut writers: Vec<JoinHandle<()>> = Vec::with_capacity(targets.len());

        for target in &targets {
            let mut child = match RaopChild::spawn(&bin, target, &anchor_file, wait_ms, latency_frames)
            {
                Ok(c) => c,
                Err(e) => {
                    // Roll back anything already spawned so we never leave orphans.
                    stop.store(true, Ordering::Release);
                    for mut c in children {
                        c.kill();
                    }
                    let _ = std::fs::remove_file(&anchor_file);
                    return Err(format!("failed to start {}: {e}", target.name));
                }
            };
            let ctl = Arc::new(DeviceControl::new(target));
            let stdin = child.take_stdin();

            if let Some(stdin) = stdin {
                let writer = spawn_writer(
                    stdin,
                    make_feed(),
                    ctl.clone(),
                    stop.clone(),
                    child.label().to_string(),
                );
                writers.push(writer);
            } else {
                log::warn!("child {} had no stdin; skipping its writer", child.label());
            }
            controls.push(ctl);
            children.push(child);
        }

        let mut active = ActiveStream {
            source_label,
            anchor_ntp,
            anchor_file,
            latency_frames,
            controls,
            children,
            stop,
            writers,
        };
        let status = active.status();
        *guard = Some(active);
        drop(guard);

        self.emit(&status);
        Ok(status)
    }

    /// Stop the active session (kill children, join writers). Idempotent.
    pub fn stop(&self) -> Result<StreamStatus, String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        if let Some(mut active) = guard.take() {
            active.shutdown();
        }
        drop(guard);
        let status = StreamStatus::idle();
        self.emit(&status);
        Ok(status)
    }

    /// Set a receiver's software volume (`0.0..=1.0`) live. Errors if no stream
    /// is active or the IP is not in the group.
    pub fn set_device_volume(&self, ip: &str, volume: f32) -> Result<StreamStatus, String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        let active = guard.as_mut().ok_or("no active stream")?;
        let ctl = active
            .controls
            .iter()
            .find(|c| c.ip == ip)
            .ok_or_else(|| format!("no target with ip {ip}"))?;
        ctl.set_gain(volume);
        let status = active.status();
        drop(guard);
        self.emit(&status);
        Ok(status)
    }

    /// Set a receiver's software delay (ms). Takes effect at the start of the
    /// stream (it shapes the silent lead-in); a mid-stream change is recorded and
    /// applies on the next start. Errors if no stream is active or IP unknown.
    pub fn set_device_delay(&self, ip: &str, delay_ms: u32) -> Result<StreamStatus, String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        let active = guard.as_mut().ok_or("no active stream")?;
        let ctl = active
            .controls
            .iter()
            .find(|c| c.ip == ip)
            .ok_or_else(|| format!("no target with ip {ip}"))?;
        ctl.set_delay_ms(delay_ms);
        let status = active.status();
        drop(guard);
        self.emit(&status);
        Ok(status)
    }

    /// Set a receiver's software delay (ms) by its persistence key (Feature 2).
    /// Same live semantics as [`set_device_delay`](Self::set_device_delay) — the
    /// value is recorded and reflected in status; it shapes the lead-in on the
    /// next start. Errors if no stream is active or the key is not in the group.
    pub fn set_target_delay(&self, key: &str, delay_ms: u32) -> Result<StreamStatus, String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        let active = guard.as_mut().ok_or("no active stream")?;
        let ctl = active
            .controls
            .iter()
            .find(|c| c.key == key)
            .ok_or_else(|| format!("no target with key {key}"))?;
        ctl.set_delay_ms(delay_ms);
        let status = active.status();
        drop(guard);
        self.emit(&status);
        Ok(status)
    }

    /// Current status (idle when nothing is streaming).
    pub fn status(&self) -> StreamStatus {
        match self.inner.lock() {
            Ok(mut guard) => match guard.as_mut() {
                Some(active) => active.status(),
                None => StreamStatus::idle(),
            },
            Err(_) => StreamStatus::idle(),
        }
    }

    fn emit(&self, status: &StreamStatus) {
        if let Ok(a) = self.app.lock() {
            if let Some(handle) = a.as_ref() {
                let _ = handle.emit("stream-state", status);
            }
        }
    }
}

/// Spawn the per-device writer thread: prepend this device's delay as silence,
/// then stream PCM to the child's stdin applying live volume. The feed is either a
/// static buffer (S2) or a live fan-out receiver (S3); the delay lead-in and
/// per-chunk volume scaling are identical for both.
fn spawn_writer(
    mut stdin: ChildStdin,
    feed: ChunkFeed,
    ctl: Arc<DeviceControl>,
    stop: Arc<AtomicBool>,
    label: String,
) -> JoinHandle<()> {
    use std::io::Write;
    std::thread::spawn(move || {
        // Per-device delay line: silent lead-in read once at start.
        let delay_frames = ms_to_frames(ctl.delay_ms());
        if delay_frames > 0 {
            let silence = silence_frames(delay_frames);
            for chunk in FrameChunks::new(&silence, CHUNK_FRAMES) {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                if stdin.write_all(chunk).is_err() {
                    return;
                }
            }
        }

        match feed {
            // S2: stream the whole buffer once. The OS pipe back-pressures here —
            // cliraop reads at real time off the shared NTP — so it self-paces.
            ChunkFeed::Static(pcm) => {
                for chunk in FrameChunks::new(&pcm, CHUNK_FRAMES) {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    if !write_chunk(&mut stdin, chunk, &ctl) {
                        break;
                    }
                }
            }
            // S3: pump live packets until stopped or the producer disconnects. A
            // short recv timeout lets the stop flag be observed even while the
            // Spotify stream is silent/paused (no packets arriving).
            ChunkFeed::Live(rx) => loop {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(packet) => {
                        if !write_chunk(&mut stdin, &packet, &ctl) {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            },
        }

        let _ = stdin.flush();
        log::debug!("writer for {label} finished ({} frames)", ctl.frames());
        // Dropping stdin here signals EOF so the child drains and exits cleanly.
    })
}

/// Write one PCM chunk to a child's stdin, applying the device's live volume.
/// Returns `false` on a broken pipe (the writer should stop). Unity gain writes
/// the shared bytes directly; any attenuation clones just this chunk to scale it.
fn write_chunk(stdin: &mut ChildStdin, chunk: &[u8], ctl: &DeviceControl) -> bool {
    use std::io::Write;
    let gain = ctl.gain();
    let ok = if gain == 1.0 {
        stdin.write_all(chunk).is_ok()
    } else {
        let mut buf = chunk.to_vec();
        scale_s16le(&mut buf, gain);
        stdin.write_all(&buf).is_ok()
    };
    if ok {
        ctl.add_frames((chunk.len() / super::model::BYTES_PER_FRAME) as u64);
    }
    ok
}
