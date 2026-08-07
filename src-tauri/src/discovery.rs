//! Device discovery (brief.md FR-4) — mDNS + SSDP + an optional subnet sweep.
//!
//! The three strategies run **concurrently** and feed one confirmation stage:
//! nothing is ever reported to the UI on the strength of an mDNS record or an
//! SSDP advert alone. Every candidate address is confirmed by actually asking it
//! `getStatusEx`, exactly like FR-5's manual add does — an AirPlay printer or a
//! smart TV answering `_airplay._tcp` must not show up as a speaker.
//!
//! Results are deduplicated by UUID (falling back to the IP when a firmware
//! answers without one), because the same unit is routinely seen by all three
//! strategies at once.
//!
//! Progress and hits stream to the frontend as events while the scan runs; the
//! `scan` command's return value is the same set, for callers that only want the
//! final answer. Emission is throttled to ≤10 Hz (NFR-5: a sweep must not spend
//! its CPU budget on IPC).
//!
//! ## Concurrency notes
//!
//! * `mdns-sd` runs a **thread**, not a task — its daemon is shut down (and
//!   awaited) on every exit path, including cancellation.
//! * SSDP is hand-rolled over `tokio::net::UdpSocket` on purpose: the ssdp-client
//!   crate drags in a second async runtime, which we will not pay for (NFR-5).
//! * The sweep is `Semaphore`-bounded at 64 in-flight probes with a short-fused
//!   HTTP client — a dead address must fail on connect, not after the poller's
//!   full 2 s budget.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ipnet::Ipv4Net;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::{Deserialize, Serialize};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter};
use tokio::net::UdpSocket;
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};

use crate::error::{AppError, AppResult};
use crate::linkplay::client::{LinkplayClient, LinkplayCommand};
use crate::linkplay::models::StatusEx;
use crate::net;
use crate::store::{self, SavedDevice};

/// Scan progress for one strategy. `total` is 0 for mDNS/SSDP, which have no
/// knowable denominator — the UI renders those as an indeterminate bar.
pub const EVENT_SCAN_PROGRESS: &str = "scan-progress";
/// A confirmed candidate, emitted the moment it answers (not batched at the end).
pub const EVENT_SCAN_DEVICE_FOUND: &str = "scan-device-found";
/// The scan finished — normally, or because it was cancelled.
pub const EVENT_SCAN_COMPLETE: &str = "scan-complete";

/// Service types LP10s advertise. `_linkplay._tcp` is the direct hit; AirPlay is
/// the wide net (every LP10 is an AirPlay receiver, and plenty of firmwares only
/// register that one) — the confirmation step filters the rest back out.
const MDNS_SERVICES: [&str; 2] = ["_linkplay._tcp.local.", "_airplay._tcp.local."];
/// How long to listen for mDNS answers. Long enough for a re-query, short enough
/// that the user isn't watching a spinner for nothing.
const MDNS_WINDOW: Duration = Duration::from_secs(4);

const SSDP_MULTICAST: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);
/// `MX` is the maximum random delay a device waits before answering, so the
/// listen window has to be comfortably larger than it.
const SSDP_MX: u32 = 2;
const SSDP_WINDOW: Duration = Duration::from_secs(3);
/// Both search targets are sent as separate M-SEARCHes: some Linkplay builds
/// only answer the generic `upnp:rootdevice`, others only the renderer type.
const SSDP_TARGETS: [&str; 2] = ["urn:schemas-upnp-org:device:MediaRenderer:1", "upnp:rootdevice"];
/// SSDP replies are small; 2 KiB truncates nothing we read.
const SSDP_BUF: usize = 2048;

/// In-flight probes. Matches the tide_pos2 printer sweep — enough to finish a
/// /24 in about a second, few enough that a cheap AP doesn't drop the flood.
const CONCURRENCY: usize = 64;
/// Sweep progress is coalesced into batches of this many hosts before emitting.
const SWEEP_BATCH: u32 = 8;
/// Floor on the gap between two progress events (≤10 Hz).
const MIN_EMIT_GAP: Duration = Duration::from_millis(100);

/// Narrowest prefix we will expand. A /16 is 65 534 probes — already generous;
/// anything broader is a typo, not an intention.
pub const MIN_SWEEP_PREFIX: u8 = 16;

// ------------------------------------------------------------------- options --

/// `scan` arguments (FR-4). The sweep is user-toggleable but defaults **on**:
/// mDNS/SSDP miss units on firmwares with broken advertising, and a /24 sweep
/// costs about a second.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScanOptions {
    pub sweep: bool,
    /// Explicit CIDR for this scan; falls back to `settings.subnet`, then to the
    /// auto-detected local /24.
    pub cidr: Option<String>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { sweep: true, cidr: None }
    }
}

/// Which strategy a progress event is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Mdns,
    Ssdp,
    Sweep,
}

impl Phase {
    const ALL: [Phase; 3] = [Phase::Mdns, Phase::Ssdp, Phase::Sweep];

    fn index(self) -> usize {
        match self {
            Phase::Mdns => 0,
            Phase::Ssdp => 1,
            Phase::Sweep => 2,
        }
    }
}

// ---------------------------------------------------------------- event DTOs --

/// A discovered device the user may add. Deliberately *not* persisted: FR-4 says
/// discovery produces candidates, and adding stays an explicit act (it goes
/// through `add_device`, which validates and starts polling).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCandidate {
    pub uuid: String,
    pub ip: String,
    /// The device's own name (candidates have no local alias yet).
    pub name: String,
    pub firmware: String,
    pub rssi: Option<i32>,
    /// Already in the saved list — the row renders as "added" rather than
    /// offering a button that would be a no-op.
    pub already_added: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanProgress {
    phase: Phase,
    scanned: u32,
    /// 0 when the strategy has no denominator (mDNS/SSDP).
    total: u32,
    found: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanFound {
    candidate: DeviceCandidate,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanComplete {
    found: u32,
    cancelled: bool,
}

// ------------------------------------------------------------- cancellation --

/// One scan's cancellation flag.
///
/// A `watch` channel rather than an `AtomicBool` + `Notify`: waiting on the
/// latter has a genuine lost-wakeup window between "check the flag" and "arm the
/// waiter", whereas `watch::Receiver::wait_for` inspects the current value
/// first. No extra dependency either way (tokio's `sync` feature is already on).
#[derive(Debug)]
pub struct ScanToken {
    tx: watch::Sender<bool>,
}

impl ScanToken {
    fn new() -> Self {
        Self { tx: watch::channel(false).0 }
    }

    /// `send_replace`, not `send`: `send` fails when no receiver is alive, and
    /// this token spends most of its life with none — a scan only subscribes at
    /// an await point. Losing that write would make cancel a silent no-op.
    pub fn cancel(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }

    /// Resolves as soon as the scan is cancelled — immediately if it already is.
    pub async fn cancelled(&self) {
        // The sender is owned by the token, so this can only fail if `self` is
        // gone, which it cannot be while we hold `&self`.
        let _ = self.tx.subscribe().wait_for(|c| *c).await;
    }
}

/// The one-scan-at-a-time slot, held in `AppState`.
///
/// **Policy: a second scan request cancels the running one and restarts.** The
/// alternative (reject with `InvalidInput`) makes the obvious user action —
/// hitting "Scan" again because the first run looked stuck — do nothing, which
/// reads as a broken button.
#[derive(Default)]
pub struct ScanControl {
    current: Mutex<Option<Arc<ScanToken>>>,
}

impl ScanControl {
    /// Claim the slot, cancelling whatever was in it.
    pub fn begin(&self) -> Arc<ScanToken> {
        let token = Arc::new(ScanToken::new());
        let mut current = self.current.lock().expect("scan lock poisoned");
        if let Some(previous) = current.replace(token.clone()) {
            previous.cancel();
        }
        token
    }

    /// Cancel the running scan. False when nothing was running.
    pub fn cancel(&self) -> bool {
        match self.current.lock().expect("scan lock poisoned").take() {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Release the slot — but only if it is still *this* scan's. A scan that was
    /// superseded must not clear the successor that replaced it.
    pub fn finish(&self, token: &Arc<ScanToken>) {
        let mut current = self.current.lock().expect("scan lock poisoned");
        if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, token)) {
            *current = None;
        }
    }
}

// ------------------------------------------------------------ pure helpers --

/// Parse and bounds-check a sweep range.
///
/// Host bits are truncated (`192.168.1.42/24` means "that /24"), which is what
/// users type when they copy their own address out of the debug pane.
pub fn parse_cidr(cidr: &str) -> AppResult<Ipv4Net> {
    let raw = cidr.trim();
    let net: Ipv4Net = raw.parse().map_err(|_| {
        AppError::InvalidInput(format!(
            "“{raw}” is not a valid IPv4 range — use CIDR notation, for example 192.168.1.0/24."
        ))
    })?;
    if net.prefix_len() < MIN_SWEEP_PREFIX {
        let addresses = 1u64 << (32 - u32::from(net.prefix_len()));
        return Err(AppError::InvalidInput(format!(
            "/{} is too broad to sweep ({addresses} addresses). Use /{MIN_SWEEP_PREFIX} or narrower.",
            net.prefix_len()
        )));
    }
    Ok(net.trunc())
}

/// The IPv4 a device is reachable at, from an SSDP reply's `LOCATION` header.
///
/// Parsed by hand rather than with a URL crate: the header is
/// `LOCATION: http://192.168.1.42:49152/description.xml` and all we want is the
/// host. Header names are case-insensitive on the wire, and some firmwares send
/// `Location:`.
pub fn parse_ssdp_location(payload: &str) -> Option<Ipv4Addr> {
    let value = payload.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("location").then(|| value.trim())
    })?;

    // http://host:port/path → host:port/path → host:port → host
    let after_scheme = value.split_once("://").map(|(_, rest)| rest).unwrap_or(value);
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    // Strip userinfo, then the port. IPv6 literals are bracketed and parse to
    // nothing here, which is correct: this app is IPv4-only on the LAN.
    let host = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    host.split(':').next()?.parse().ok()
}

/// Dedupe key. The UUID is the identity everywhere else in the app; the IP is
/// only a fallback for a firmware that answers `getStatusEx` without one.
pub fn candidate_key(uuid: &str, ip: &str) -> String {
    let uuid = uuid.trim();
    if uuid.is_empty() {
        format!("ip:{ip}")
    } else {
        format!("uuid:{uuid}")
    }
}

/// Is this candidate already in the user's saved list (FR-4 dedupe)?
///
/// Matched on UUID, so a speaker that moved to a new IP is still "added" — the
/// IP refresh is `add_device`'s job, not a reason to offer it as new.
pub fn already_added(uuid: &str, ip: &str, saved: &[SavedDevice]) -> bool {
    let uuid = uuid.trim();
    saved.iter().any(|d| {
        if uuid.is_empty() || d.uuid.is_empty() {
            d.ip == ip
        } else {
            d.uuid == uuid
        }
    })
}

/// Turn a confirmed `getStatusEx` answer into a candidate.
///
/// `None` when the box answered but is not something we can add: without a UUID
/// there is no identity to persist, and `add_device` would reject it anyway.
pub fn candidate_from(ip: &str, status: &StatusEx, saved: &[SavedDevice]) -> Option<DeviceCandidate> {
    let uuid = status.uuid.trim().to_string();
    if uuid.is_empty() {
        return None;
    }
    Some(DeviceCandidate {
        already_added: already_added(&uuid, ip, saved),
        uuid,
        ip: ip.to_string(),
        name: status.device_name.trim().to_string(),
        firmware: status.firmware.trim().to_string(),
        rssi: status.rssi,
    })
}

// ------------------------------------------------------------------ the run --

#[derive(Debug, Clone, Copy, Default)]
struct Counters {
    scanned: u32,
    total: u32,
}

#[derive(Debug, Default)]
struct Progress {
    counters: [Counters; 3],
    found: u32,
    last_emit: Option<Instant>,
}

/// Everything one scan shares between its strategies.
struct ScanRun {
    app: AppHandle,
    token: Arc<ScanToken>,
    /// Short-fused client: the sweep touches every address in the subnet, so a
    /// dead one has to fail fast rather than burn the poller's 2 s budget.
    client: LinkplayClient,
    /// Snapshot of the saved list taken once, at the start — the dedupe basis.
    saved: Vec<SavedDevice>,
    /// Addresses already handed to a probe, so the three strategies (and the
    /// sweep's overlap with them) never probe the same host twice.
    seen: Mutex<HashSet<Ipv4Addr>>,
    /// Confirmed candidates, keyed by [`candidate_key`].
    found: Mutex<HashMap<String, DeviceCandidate>>,
    progress: Mutex<Progress>,
    permits: Arc<Semaphore>,
    probes: Mutex<Vec<JoinHandle<()>>>,
}

impl ScanRun {
    /// Claim an address. False when someone already took it.
    fn claim(&self, ip: Ipv4Addr) -> bool {
        self.seen.lock().expect("scan seen lock poisoned").insert(ip)
    }

    /// Probe `ip` on a bounded background task. Returns false when the address
    /// was already claimed (no task spawned, so the caller owns the accounting).
    ///
    /// The permit is acquired *before* spawning so a /16 doesn't materialise
    /// 65k tasks at once.
    async fn probe(self: &Arc<Self>, ip: Ipv4Addr, phase: Phase) -> bool {
        if self.token.is_cancelled() || !self.claim(ip) {
            return false;
        }
        let Ok(permit) = self.permits.clone().acquire_owned().await else {
            return false; // Semaphore closed — only happens if we are tearing down.
        };
        let run = self.clone();
        let handle = tauri::async_runtime::spawn(async move {
            let _permit: OwnedSemaphorePermit = permit;
            run.confirm(ip).await;
            run.scanned(phase);
        });
        self.probes.lock().expect("scan probe lock poisoned").push(handle);
        true
    }

    /// Ask the address `getStatusEx` and record it if it answers like an LP10.
    async fn confirm(&self, ip: Ipv4Addr) {
        if self.token.is_cancelled() {
            return;
        }
        let ip = ip.to_string();
        let status: StatusEx = match self.client.send_json(&ip, &LinkplayCommand::GetStatusEx).await {
            Ok(status) => status,
            // Unreachable/not-a-Linkplay-box is the *expected* answer for almost
            // every address in a sweep — never surface it.
            Err(e) => return log::trace!("{ip}: not a device: {e}"),
        };
        let Some(candidate) = candidate_from(&ip, &status, &self.saved) else {
            return log::debug!("{ip} answered getStatusEx without a uuid — skipping");
        };
        self.record(candidate);
    }

    fn record(&self, candidate: DeviceCandidate) {
        let key = candidate_key(&candidate.uuid, &candidate.ip);
        {
            let mut found = self.found.lock().expect("scan found lock poisoned");
            // First sighting wins: the strategies race, and re-emitting the same
            // unit would make the results list flicker.
            if found.contains_key(&key) {
                return;
            }
            found.insert(key, candidate.clone());
        }
        self.progress.lock().expect("scan progress lock poisoned").found += 1;
        self.emit(EVENT_SCAN_DEVICE_FOUND, &ScanFound { candidate });
    }

    fn set_total(self: &Arc<Self>, phase: Phase, total: u32) {
        self.progress.lock().expect("scan progress lock poisoned").counters[phase.index()].total = total;
        self.publish(phase, true);
    }

    /// One address finished being probed.
    fn scanned(&self, phase: Phase) {
        self.progress.lock().expect("scan progress lock poisoned").counters[phase.index()].scanned += 1;
        self.publish(phase, false);
    }

    /// Emit a progress event, subject to the batching/rate rules.
    fn publish(&self, phase: Phase, force: bool) {
        let payload = {
            let mut progress = self.progress.lock().expect("scan progress lock poisoned");
            let counters = progress.counters[phase.index()];
            let complete = counters.total > 0 && counters.scanned >= counters.total;
            // The sweep produces one update per host; coalesce them, then apply
            // the global rate limit on top (≤10 Hz).
            let batched = phase != Phase::Sweep || counters.scanned % SWEEP_BATCH == 0;
            let due = progress.last_emit.is_none_or(|t| t.elapsed() >= MIN_EMIT_GAP);
            if !(force || complete || (batched && due)) {
                return;
            }
            progress.last_emit = Some(Instant::now());
            ScanProgress {
                phase,
                scanned: counters.scanned,
                total: counters.total,
                found: progress.found,
            }
        };
        self.emit(EVENT_SCAN_PROGRESS, &payload);
    }

    fn emit<T: Serialize + Clone>(&self, event: &str, payload: &T) {
        if let Err(e) = self.app.emit(event, payload) {
            log::error!("failed to emit {event}: {e}");
        }
    }
}

/// Run a full scan. Returns the confirmed candidates; the same information has
/// already streamed out as events by the time this resolves.
pub async fn run(
    app: AppHandle,
    options: ScanOptions,
    token: Arc<ScanToken>,
) -> AppResult<Vec<DeviceCandidate>> {
    let config = store::get(&app);

    // Resolve (and validate) the sweep range *first*: a bad CIDR must fail the
    // command outright, before any event claims a scan started.
    let sweep_net = if options.sweep {
        match sweep_cidr(&options, config.settings.subnet.as_deref()) {
            Some(cidr) => Some(parse_cidr(&cidr)?),
            None => {
                log::warn!("sweep requested but no local IPv4 could be detected — skipping it");
                None
            }
        }
    } else {
        None
    };

    let run = Arc::new(ScanRun {
        app: app.clone(),
        token,
        client: LinkplayClient::probe(),
        saved: config.devices,
        seen: Mutex::new(HashSet::new()),
        found: Mutex::new(HashMap::new()),
        progress: Mutex::new(Progress::default()),
        permits: Arc::new(Semaphore::new(CONCURRENCY)),
        probes: Mutex::new(Vec::new()),
    });

    // Announce every phase up front so the UI can render the whole plan rather
    // than growing a row per strategy as each one starts.
    for phase in Phase::ALL {
        if phase != Phase::Sweep || sweep_net.is_some() {
            run.publish(phase, true);
        }
    }

    tokio::join!(browse_mdns(&run), browse_ssdp(&run), async {
        if let Some(net) = sweep_net {
            sweep(&run, net).await;
        }
    });

    // Drain the in-flight probes: a candidate found on the last host of the
    // sweep must still make it into the returned list.
    let probes = std::mem::take(&mut *run.probes.lock().expect("scan probe lock poisoned"));
    for probe in probes {
        let _ = probe.await;
    }

    let cancelled = run.token.is_cancelled();
    let mut candidates: Vec<DeviceCandidate> = run
        .found
        .lock()
        .expect("scan found lock poisoned")
        .values()
        .cloned()
        .collect();
    // Address order, so the list doesn't depend on which strategy won the race.
    candidates.sort_by_key(|c| c.ip.parse::<Ipv4Addr>().map(|ip| ip.octets()).unwrap_or([255; 4]));

    run.emit(
        EVENT_SCAN_COMPLETE,
        &ScanComplete { found: candidates.len() as u32, cancelled },
    );
    Ok(candidates)
}

/// The CIDR to sweep: the request wins, then the saved setting, then the
/// auto-detected local /24 (FR-4).
fn sweep_cidr(options: &ScanOptions, configured: Option<&str>) -> Option<String> {
    let requested = options.cidr.as_deref().map(str::trim).filter(|c| !c.is_empty());
    requested
        .or_else(|| configured.map(str::trim).filter(|c| !c.is_empty()))
        .map(str::to_string)
        .or_else(net::local_cidr24)
}

// ------------------------------------------------------------------- mDNS --

async fn browse_mdns(run: &Arc<ScanRun>) {
    let daemon = match ServiceDaemon::new() {
        Ok(daemon) => daemon,
        // No multicast socket (locked-down VM, no interface) — the other two
        // strategies still stand on their own.
        Err(e) => return log::warn!("mDNS unavailable: {e}"),
    };

    let mut listeners = Vec::new();
    for service in MDNS_SERVICES {
        match daemon.browse(service) {
            Ok(rx) => {
                let run = run.clone();
                listeners.push(tauri::async_runtime::spawn(async move {
                    while let Ok(event) = rx.recv_async().await {
                        if let ServiceEvent::ServiceResolved(info) = event {
                            for ip in info.get_addresses_v4() {
                                run.probe(*ip, Phase::Mdns).await;
                            }
                        }
                    }
                }));
            }
            Err(e) => log::warn!("mDNS browse of {service} failed: {e}"),
        }
    }

    tokio::select! {
        _ = tokio::time::sleep(MDNS_WINDOW) => {}
        _ = run.token.cancelled() => {}
    }

    for listener in listeners {
        listener.abort();
    }
    // The daemon owns an OS thread; dropping the handle would leave it running
    // for the life of the app. Wait for it to confirm it is down.
    match daemon.shutdown() {
        Ok(status) => {
            let _ = status.recv_async().await;
        }
        Err(e) => log::warn!("mDNS daemon shutdown failed: {e}"),
    }
}

// ------------------------------------------------------------------- SSDP --

async fn browse_ssdp(run: &Arc<ScanRun>) {
    let socket = match UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0))).await {
        Ok(socket) => socket,
        Err(e) => return log::warn!("SSDP socket unavailable: {e}"),
    };
    // TTL 2 so the search crosses one bridge/AP hop; higher would leak the
    // search past the LAN for no benefit (NFR-1).
    if let Err(e) = socket.set_multicast_ttl_v4(2) {
        log::debug!("could not set the SSDP multicast TTL: {e}");
    }

    for target in SSDP_TARGETS {
        let search = format!(
            "M-SEARCH * HTTP/1.1\r\n\
             HOST: {SSDP_MULTICAST}\r\n\
             MAN: \"ssdp:discover\"\r\n\
             MX: {SSDP_MX}\r\n\
             ST: {target}\r\n\r\n"
        );
        if let Err(e) = socket.send_to(search.as_bytes(), SSDP_MULTICAST).await {
            // One refused send usually means no route for multicast at all.
            return log::warn!("SSDP M-SEARCH could not be sent: {e}");
        }
    }

    let mut buf = vec![0u8; SSDP_BUF];
    let deadline = tokio::time::Instant::now() + SSDP_WINDOW;
    loop {
        let received = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            _ = run.token.cancelled() => break,
            received = socket.recv_from(&mut buf) => received,
        };
        let (len, from) = match received {
            Ok(received) => received,
            Err(e) => {
                log::debug!("SSDP receive failed: {e}");
                break;
            }
        };

        // Replies are unicast back to us, so the sender is the device — a usable
        // fallback for the firmwares that send a hostname in LOCATION.
        let text = String::from_utf8_lossy(&buf[..len]);
        let ip = parse_ssdp_location(&text).or(match from.ip() {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        });
        if let Some(ip) = ip {
            run.probe(ip, Phase::Ssdp).await;
        }
    }
}

// ------------------------------------------------------------------ sweep --

async fn sweep(run: &Arc<ScanRun>, net: Ipv4Net) {
    let local = net::local_ipv4();
    let hosts: Vec<Ipv4Addr> = net.hosts().filter(|host| Some(*host) != local).collect();
    run.set_total(Phase::Sweep, hosts.len() as u32);

    for host in hosts {
        if run.token.is_cancelled() {
            break;
        }
        // Already claimed by mDNS/SSDP: no probe runs, so count it here or the
        // bar would never reach its total.
        if !run.probe(host, Phase::Sweep).await {
            run.scanned(Phase::Sweep);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(uuid: &str, ip: &str) -> SavedDevice {
        SavedDevice { uuid: uuid.into(), ip: ip.into(), ..SavedDevice::default() }
    }

    // ------------------------------------------------------------- CIDR --

    #[test]
    fn cidr_accepts_a_plain_network() {
        let net = parse_cidr("192.168.10.0/24").expect("a /24 must parse");
        assert_eq!(net.to_string(), "192.168.10.0/24");
        assert_eq!(net.hosts().count(), 254);
    }

    #[test]
    fn cidr_truncates_host_bits() {
        // Users paste their own address out of the debug pane.
        let net = parse_cidr(" 192.168.10.47/24 ").expect("host bits must be tolerated");
        assert_eq!(net.to_string(), "192.168.10.0/24");
    }

    #[test]
    fn cidr_rejects_garbage_and_bare_addresses() {
        for bad in ["", "not a subnet", "192.168.1.0", "192.168.1.0/33", "10.0.0.0/-1"] {
            let err = parse_cidr(bad).expect_err("must be rejected");
            assert_eq!(err.code(), "invalid_input", "{bad}");
        }
    }

    #[test]
    fn cidr_caps_the_sweep_width() {
        assert!(parse_cidr("10.0.0.0/16").is_ok(), "/16 is the widest allowed");
        for wide in ["10.0.0.0/15", "10.0.0.0/8", "0.0.0.0/0"] {
            let err = parse_cidr(wide).expect_err("must be rejected as too broad");
            assert_eq!(err.code(), "invalid_input", "{wide}");
        }
    }

    #[test]
    fn sweep_cidr_prefers_request_then_setting_then_auto() {
        let requested = ScanOptions { sweep: true, cidr: Some("10.1.2.0/24".into()) };
        assert_eq!(
            sweep_cidr(&requested, Some("192.168.0.0/24")).as_deref(),
            Some("10.1.2.0/24")
        );

        let blank = ScanOptions { sweep: true, cidr: Some("   ".into()) };
        assert_eq!(
            sweep_cidr(&blank, Some("192.168.0.0/24")).as_deref(),
            Some("192.168.0.0/24"),
            "a blank input must not shadow the saved setting"
        );

        // With neither, it falls through to auto-detection, which depends on the
        // host's routing table — only assert that it is well-formed if present.
        let auto = sweep_cidr(&ScanOptions::default(), None);
        if let Some(cidr) = auto {
            assert!(parse_cidr(&cidr).is_ok(), "auto-detected {cidr} must be sweepable");
        }
    }

    // ------------------------------------------------------------- SSDP --

    #[test]
    fn ssdp_location_yields_the_host_ip() {
        let reply = "HTTP/1.1 200 OK\r\n\
                     CACHE-CONTROL: max-age=1800\r\n\
                     LOCATION: http://192.168.10.42:49152/description.xml\r\n\
                     ST: upnp:rootdevice\r\n\r\n";
        assert_eq!(parse_ssdp_location(reply), Some(Ipv4Addr::new(192, 168, 10, 42)));
    }

    #[test]
    fn ssdp_location_header_is_case_insensitive_and_port_optional() {
        assert_eq!(
            parse_ssdp_location("Location: http://10.0.0.5/desc.xml\r\n"),
            Some(Ipv4Addr::new(10, 0, 0, 5))
        );
        assert_eq!(
            parse_ssdp_location("location:http://10.0.0.6:8080\r\n"),
            Some(Ipv4Addr::new(10, 0, 0, 6))
        );
    }

    #[test]
    fn ssdp_location_ignores_what_it_cannot_use() {
        // No LOCATION at all, a hostname instead of an IP, and an IPv6 literal —
        // all fall back to the sender address at the call site.
        assert_eq!(parse_ssdp_location("HTTP/1.1 200 OK\r\nST: upnp:rootdevice\r\n"), None);
        assert_eq!(parse_ssdp_location("LOCATION: http://lp10.local:49152/d.xml\r\n"), None);
        assert_eq!(parse_ssdp_location("LOCATION: http://[fe80::1]:49152/d.xml\r\n"), None);
    }

    #[test]
    fn ssdp_location_skips_the_other_headers() {
        // `SERVER:` also contains a colon and must not be mistaken for the one
        // header we want.
        let reply = "HTTP/1.1 200 OK\r\n\
                     SERVER: Linux/3.10 UPnP/1.0 Linkplay/4.6\r\n\
                     USN: uuid:FF31F09E::upnp:rootdevice\r\n\
                     LOCATION: http://172.16.4.9:49152/description.xml\r\n\r\n";
        assert_eq!(parse_ssdp_location(reply), Some(Ipv4Addr::new(172, 16, 4, 9)));
    }

    // ----------------------------------------------------------- dedupe --

    #[test]
    fn candidate_key_prefers_uuid_over_ip() {
        assert_eq!(candidate_key("FF31", "192.168.1.4"), candidate_key(" FF31 ", "192.168.1.9"));
        assert_ne!(candidate_key("", "192.168.1.4"), candidate_key("", "192.168.1.9"));
    }

    #[test]
    fn already_added_matches_on_uuid_across_a_new_ip() {
        let list = [saved("FF31", "192.168.1.4")];
        assert!(already_added("FF31", "192.168.1.4", &list));
        assert!(
            already_added("FF31", "192.168.1.77", &list),
            "a DHCP move is still the same saved device"
        );
        assert!(!already_added("AB01", "192.168.1.4", &list), "a different unit on a reused IP is new");
    }

    #[test]
    fn already_added_falls_back_to_the_ip_without_a_uuid() {
        let list = [saved("", "192.168.1.4")];
        assert!(already_added("FF31", "192.168.1.4", &list));
        assert!(!already_added("FF31", "192.168.1.5", &list));
    }

    #[test]
    fn candidate_from_marks_saved_devices_and_drops_uuidless_answers() {
        let list = [saved("FF31", "192.168.1.4")];
        let status: StatusEx = serde_json::from_str(
            r#"{"uuid":"FF31","DeviceName":"Kitchen","firmware":"4.6.415145","RSSI":"-52"}"#,
        )
        .unwrap();
        let candidate = candidate_from("192.168.1.4", &status, &list).expect("must be a candidate");
        assert_eq!(candidate.uuid, "FF31");
        assert_eq!(candidate.name, "Kitchen");
        assert_eq!(candidate.firmware, "4.6.415145");
        assert_eq!(candidate.rssi, Some(-52));
        assert!(candidate.already_added);

        let fresh: StatusEx = serde_json::from_str(r#"{"uuid":"AB01","DeviceName":"Bath"}"#).unwrap();
        let candidate = candidate_from("192.168.1.9", &fresh, &list).expect("must be a candidate");
        assert!(!candidate.already_added);

        // A box that answers but has no identity cannot be persisted.
        let anonymous: StatusEx = serde_json::from_str(r#"{"DeviceName":"Some TV"}"#).unwrap();
        assert!(candidate_from("192.168.1.20", &anonymous, &list).is_none());
    }

    #[test]
    fn scan_options_default_to_a_sweep() {
        // FR-4's "optional" means user-toggleable, not off — mDNS/SSDP alone
        // miss units whose advertising is broken.
        assert!(ScanOptions::default().sweep);
        let parsed: ScanOptions = serde_json::from_str("{}").unwrap();
        assert!(parsed.sweep);
        let off: ScanOptions = serde_json::from_str(r#"{"sweep":false,"cidr":"10.0.0.0/24"}"#).unwrap();
        assert!(!off.sweep);
        assert_eq!(off.cidr.as_deref(), Some("10.0.0.0/24"));
    }

    // ----------------------------------------------------- cancellation --

    #[test]
    fn scan_control_restarts_rather_than_refusing() {
        let control = ScanControl::default();
        let first = control.begin();
        let second = control.begin();
        assert!(first.is_cancelled(), "a second scan must cancel the first");
        assert!(!second.is_cancelled());

        // The superseded scan finishing must not clear the live one.
        control.finish(&first);
        assert!(control.cancel(), "the second scan is still the current one");
        assert!(second.is_cancelled());
        assert!(!control.cancel(), "nothing is running any more");
    }

    /// Bounded on purpose: the failure mode this guards against (a cancel that
    /// never lands — `watch::Sender::send` refuses to write when no receiver is
    /// alive, which is this token's normal state) hangs rather than asserts.
    #[tokio::test]
    async fn cancelled_resolves_for_waiters_and_latecomers() {
        let token = Arc::new(ScanToken::new());
        let waiter = {
            let token = token.clone();
            tokio::spawn(async move { token.cancelled().await })
        };
        // Let the waiter reach its await before flipping the flag.
        tokio::task::yield_now().await;
        token.cancel();

        let deadline = Duration::from_secs(5);
        tokio::time::timeout(deadline, waiter)
            .await
            .expect("an in-flight waiter must be woken")
            .expect("the waiting task must not panic");
        // And a wait started after the fact returns immediately.
        tokio::time::timeout(deadline, token.cancelled())
            .await
            .expect("a late waiter must see the flag already set");
    }
}
