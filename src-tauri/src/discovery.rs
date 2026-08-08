//! Device discovery (FR-4), rebased onto Luci/DDMS.
//!
//! Three strategies feed one confirmation stage; nothing reaches the UI on the
//! strength of a raw advert:
//!
//! * **DDMS M-SEARCH** (primary): UDP multicast `239.255.255.250:1800`,
//!   `ST: urn:schemas-upnp-org:device:DDMSServer:1`. The reply is a CRLF
//!   `KEY:VALUE` banner that yields identity, the Luci port, the wired/Wi-Fi
//!   distinction and group topology in one packet.
//! * **SSDP MediaRenderer** (identity): UDP multicast `239.255.255.250:1900`,
//!   `ST: urn:schemas-upnp-org:device:MediaRenderer:1` → fetch `description.xml`
//!   for the stable UPnP **UDN uuid**, which becomes the device's key.
//! * **Subnet sweep** (fallback): probe `<ip>:7777` over TLS and confirm with
//!   `DevInfo(92)`. `Semaphore`-bounded at 64, like the old sweep.
//!
//! Candidates are confirmed by a DDMS banner **or** a Luci `DevInfo` — never
//! httpapi. Progress/hits stream as events; cancellation is a `watch` flag.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};

use crate::error::{AppError, AppResult};
use crate::luci::client::LUCI_PORT;
use crate::luci::messagebox::MessageBox;
use crate::luci::model::{DdmsBanner, DevInfo, NetMode};
use crate::luci::LuciClient;
use crate::net;
use crate::store::{self, SavedDevice};

/// Scan progress for one strategy. `total` is 0 for DDMS/SSDP (no denominator).
pub const EVENT_SCAN_PROGRESS: &str = "scan-progress";
/// A confirmed candidate, emitted as soon as it is assembled.
pub const EVENT_SCAN_DEVICE_FOUND: &str = "scan-device-found";
/// The scan finished — normally, or because it was cancelled.
pub const EVENT_SCAN_COMPLETE: &str = "scan-complete";

const MULTICAST_V4: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
/// DDMS discovery target.
const DDMS_PORT: u16 = 1800;
const DDMS_ST: &str = "urn:schemas-upnp-org:device:DDMSServer:1";
/// Standard SSDP.
const SSDP_PORT: u16 = 1900;
const SSDP_ST: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
/// Default UPnP description port when a LOCATION header omits it.
const UPNP_PORT: u16 = 49494;

/// `MX` — the max random delay a device waits before replying.
const MX: u32 = 2;
/// Listen windows: comfortably larger than `MX`.
const DDMS_WINDOW: Duration = Duration::from_secs(3);
const SSDP_WINDOW: Duration = Duration::from_secs(3);
/// Datagram/description buffers.
const UDP_BUF: usize = 4096;

/// In-flight sweep probes.
const CONCURRENCY: usize = 64;
const SWEEP_BATCH: u32 = 8;
const MIN_EMIT_GAP: Duration = Duration::from_millis(100);
/// TLS confirm budget per swept host.
const SWEEP_TLS_TIMEOUT: Duration = Duration::from_millis(1200);
/// A dead `:7777` must fail on connect, not hang the sweep.
const SWEEP_CONNECT_TIMEOUT: Duration = Duration::from_millis(400);
/// HTTP fetch budget for `description.xml`.
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);

/// Narrowest prefix we will expand (a /16 is already 65 534 probes).
pub const MIN_SWEEP_PREFIX: u8 = 16;

// ------------------------------------------------------------------- options --

/// `scan` arguments (FR-4). The sweep is user-toggleable but defaults **on**.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScanOptions {
    pub sweep: bool,
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
    Ddms,
    Ssdp,
    Sweep,
}

impl Phase {
    const ALL: [Phase; 3] = [Phase::Ddms, Phase::Ssdp, Phase::Sweep];

    fn index(self) -> usize {
        match self {
            Phase::Ddms => 0,
            Phase::Ssdp => 1,
            Phase::Sweep => 2,
        }
    }
}

// ---------------------------------------------------------------- event DTOs --

/// A discovered device the user may add. Not persisted — adding goes through
/// `add_device`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCandidate {
    /// UPnP UDN uuid — the stable key (empty when only DDMS/Luci confirmed it).
    pub uuid: String,
    /// DDMS `USN` (a MAC), the fallback key.
    pub usn: String,
    pub ip: String,
    pub name: String,
    pub model: String,
    pub firmware: String,
    pub net_mode: Option<NetMode>,
    pub wifi_band: Option<String>,
    /// Already saved — matched on uuid, then usn, then ip.
    pub already_added: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanProgress {
    phase: Phase,
    scanned: u32,
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

/// UPnP identity from `description.xml`.
#[derive(Debug, Clone, Default)]
struct UpnpIdentity {
    uuid: String,
    model: String,
    friendly_name: String,
}

// ------------------------------------------------------------- cancellation --

/// One scan's cancellation flag (a `watch`, which has no lost-wakeup window).
#[derive(Debug)]
pub struct ScanToken {
    tx: watch::Sender<bool>,
}

impl ScanToken {
    fn new() -> Self {
        Self { tx: watch::channel(false).0 }
    }

    pub fn cancel(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }

    pub async fn cancelled(&self) {
        let _ = self.tx.subscribe().wait_for(|c| *c).await;
    }
}

/// The one-scan-at-a-time slot. A second request cancels the running one and
/// restarts — the obvious user action must not read as a dead button.
#[derive(Default)]
pub struct ScanControl {
    current: Mutex<Option<Arc<ScanToken>>>,
}

impl ScanControl {
    pub fn begin(&self) -> Arc<ScanToken> {
        let token = Arc::new(ScanToken::new());
        let mut current = self.current.lock().expect("scan lock poisoned");
        if let Some(previous) = current.replace(token.clone()) {
            previous.cancel();
        }
        token
    }

    pub fn cancel(&self) -> bool {
        match self.current.lock().expect("scan lock poisoned").take() {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    pub fn finish(&self, token: &Arc<ScanToken>) {
        let mut current = self.current.lock().expect("scan lock poisoned");
        if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, token)) {
            *current = None;
        }
    }
}

// ------------------------------------------------------------ pure helpers --

/// Parse and bounds-check a sweep range. Host bits are truncated.
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

/// The host+port+path a device's `description.xml` lives at, from an SSDP reply's
/// `LOCATION` header. Falls back to :49494 / `/description.xml`.
fn location_target(payload: &str) -> Option<(Ipv4Addr, u16, String)> {
    let value = payload.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("location").then(|| value.trim())
    })?;
    let after_scheme = value.split_once("://").map(|(_, rest)| rest).unwrap_or(value);
    let (authority, path) = match after_scheme.find('/') {
        Some(i) => (&after_scheme[..i], &after_scheme[i..]),
        None => (after_scheme, "/description.xml"),
    };
    let authority = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().unwrap_or(UPNP_PORT)),
        None => (authority, UPNP_PORT),
    };
    let ip: Ipv4Addr = host.parse().ok()?;
    Some((ip, port, path.to_string()))
}

/// The sender IP of an SSDP/DDMS reply, as a fallback when there is no LOCATION.
pub fn reply_ip(from: SocketAddr) -> Option<Ipv4Addr> {
    match from.ip() {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(_) => None,
    }
}

/// A single value out of an XML-ish document: the text between `<tag>` and
/// `</tag>` (case-sensitive, first match). Good enough for `description.xml`.
fn xml_field(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}

/// Dedupe/identity key: uuid, then usn, then ip.
pub fn candidate_key(uuid: &str, usn: &str, ip: &str) -> String {
    let uuid = uuid.trim();
    let usn = usn.trim();
    if !uuid.is_empty() {
        format!("uuid:{uuid}")
    } else if !usn.is_empty() {
        format!("usn:{}", usn.to_ascii_lowercase())
    } else {
        format!("ip:{ip}")
    }
}

/// Is this candidate already saved (matched on uuid, then usn, then ip)?
pub fn already_added(uuid: &str, usn: &str, ip: &str, saved: &[SavedDevice]) -> bool {
    let uuid = uuid.trim();
    let usn = usn.trim();
    saved.iter().any(|d| {
        (!uuid.is_empty() && d.uuid == uuid)
            || (!usn.is_empty() && d.usn.eq_ignore_ascii_case(usn))
            || d.ip == ip
    })
}

// ------------------------------------------------------------------- the run --

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

/// Everything one scan shares.
struct ScanRun {
    app: AppHandle,
    token: Arc<ScanToken>,
    saved: Vec<SavedDevice>,
    /// DDMS banners by IP (primary confirmation).
    banners: Mutex<HashMap<Ipv4Addr, DdmsBanner>>,
    /// UPnP identities by IP (for the stable uuid).
    identities: Mutex<HashMap<Ipv4Addr, UpnpIdentity>>,
    /// Confirmed candidates, keyed by [`candidate_key`].
    found: Mutex<HashMap<String, DeviceCandidate>>,
    progress: Mutex<Progress>,
    permits: Arc<Semaphore>,
}

impl ScanRun {
    fn record_candidate(&self, candidate: DeviceCandidate) {
        let key = candidate_key(&candidate.uuid, &candidate.usn, &candidate.ip);
        {
            let mut found = self.found.lock().expect("found lock poisoned");
            if found.contains_key(&key) {
                return;
            }
            found.insert(key, candidate.clone());
        }
        self.progress.lock().expect("progress lock poisoned").found += 1;
        self.emit(EVENT_SCAN_DEVICE_FOUND, &ScanFound { candidate });
    }

    fn set_total(&self, phase: Phase, total: u32) {
        self.progress.lock().expect("progress lock poisoned").counters[phase.index()].total = total;
        self.publish(phase, true);
    }

    fn scanned(&self, phase: Phase) {
        self.progress.lock().expect("progress lock poisoned").counters[phase.index()].scanned += 1;
        self.publish(phase, false);
    }

    fn publish(&self, phase: Phase, force: bool) {
        let payload = {
            let mut progress = self.progress.lock().expect("progress lock poisoned");
            let counters = progress.counters[phase.index()];
            let complete = counters.total > 0 && counters.scanned >= counters.total;
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

    /// Build the candidate for a DDMS-confirmed IP, attaching the UPnP uuid.
    fn ddms_candidate(&self, ip: Ipv4Addr, banner: &DdmsBanner) -> DeviceCandidate {
        let identity = self.identities.lock().expect("identities lock poisoned").get(&ip).cloned().unwrap_or_default();
        let usn = banner.usn().unwrap_or("").to_string();
        let name = banner
            .device_name()
            .map(str::to_string)
            .filter(|n| !n.is_empty())
            .or_else(|| Some(identity.friendly_name.clone()).filter(|n| !n.is_empty()))
            .unwrap_or_default();
        let model = banner
            .model()
            .map(str::to_string)
            .filter(|m| !m.is_empty())
            .unwrap_or(identity.model.clone());
        DeviceCandidate {
            already_added: already_added(&identity.uuid, &usn, &ip.to_string(), &self.saved),
            uuid: identity.uuid,
            usn,
            ip: ip.to_string(),
            name,
            model,
            firmware: banner.firmware().unwrap_or("").to_string(),
            net_mode: banner.net_mode(),
            wifi_band: banner.wifi_band().map(str::to_string),
        }
    }
}

/// Run a full scan. Returns the confirmed candidates; the same information has
/// already streamed out as events.
pub async fn run(
    app: AppHandle,
    options: ScanOptions,
    token: Arc<ScanToken>,
) -> AppResult<Vec<DeviceCandidate>> {
    let config = store::get(&app);

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
        saved: config.devices,
        banners: Mutex::new(HashMap::new()),
        identities: Mutex::new(HashMap::new()),
        found: Mutex::new(HashMap::new()),
        progress: Mutex::new(Progress::default()),
        permits: Arc::new(Semaphore::new(CONCURRENCY)),
    });

    for phase in Phase::ALL {
        if phase != Phase::Sweep || sweep_net.is_some() {
            run.publish(phase, true);
        }
    }

    // DDMS + SSDP listen concurrently (both ~3s). SSDP fills identities; DDMS
    // fills banners. We assemble candidates once both settle so every one
    // carries its stable uuid.
    tokio::join!(collect_ddms(&run), collect_ssdp(&run));

    if !run.token.is_cancelled() {
        let banners: Vec<(Ipv4Addr, DdmsBanner)> = run
            .banners
            .lock()
            .expect("banners lock poisoned")
            .iter()
            .map(|(ip, b)| (*ip, b.clone()))
            .collect();
        for (ip, banner) in banners {
            let candidate = run.ddms_candidate(ip, &banner);
            run.record_candidate(candidate);
        }
    }

    // Sweep the hosts DDMS didn't already confirm.
    if let Some(net) = sweep_net {
        sweep(&run, net).await;
    }

    let cancelled = run.token.is_cancelled();
    let mut candidates: Vec<DeviceCandidate> =
        run.found.lock().expect("found lock poisoned").values().cloned().collect();
    candidates.sort_by_key(|c| c.ip.parse::<Ipv4Addr>().map(|ip| ip.octets()).unwrap_or([255; 4]));

    run.emit(
        EVENT_SCAN_COMPLETE,
        &ScanComplete { found: candidates.len() as u32, cancelled },
    );
    Ok(candidates)
}

fn sweep_cidr(options: &ScanOptions, configured: Option<&str>) -> Option<String> {
    let requested = options.cidr.as_deref().map(str::trim).filter(|c| !c.is_empty());
    requested
        .or_else(|| configured.map(str::trim).filter(|c| !c.is_empty()))
        .map(str::to_string)
        .or_else(net::local_cidr24)
}

// ------------------------------------------------------------------- DDMS --

/// Send an M-SEARCH to a multicast group and return the bound socket.
async fn msearch_socket(target: &str, port: u16) -> AppResult<UdpSocket> {
    let socket = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0)))
        .await
        .map_err(|e| AppError::Internal(format!("UDP socket unavailable: {e}")))?;
    let _ = socket.set_multicast_ttl_v4(2);
    let dest = SocketAddrV4::new(MULTICAST_V4, port);
    let search = format!(
        "M-SEARCH * HTTP/1.1\r\n\
         HOST: {dest}\r\n\
         MAN: \"ssdp:discover\"\r\n\
         MX: {MX}\r\n\
         ST: {target}\r\n\r\n"
    );
    // Send twice — some firmwares miss the first datagram.
    for _ in 0..2 {
        socket
            .send_to(search.as_bytes(), dest)
            .await
            .map_err(|e| AppError::Internal(format!("M-SEARCH could not be sent: {e}")))?;
    }
    Ok(socket)
}

async fn collect_ddms(run: &Arc<ScanRun>) {
    let socket = match msearch_socket(DDMS_ST, DDMS_PORT).await {
        Ok(socket) => socket,
        Err(e) => return log::warn!("DDMS discovery: {e}"),
    };

    let mut buf = vec![0u8; UDP_BUF];
    let deadline = tokio::time::Instant::now() + DDMS_WINDOW;
    loop {
        let received = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            _ = run.token.cancelled() => break,
            received = socket.recv_from(&mut buf) => received,
        };
        let (len, from) = match received {
            Ok(v) => v,
            Err(e) => {
                log::debug!("DDMS receive failed: {e}");
                break;
            }
        };
        let Some(ip) = reply_ip(from) else { continue };
        let text = String::from_utf8_lossy(&buf[..len]);
        let banner = DdmsBanner::parse(&text);
        // A DDMS banner names the device port; require it to look like a device.
        if banner.device_name().is_some() || banner.port().is_some() || banner.usn().is_some() {
            run.banners.lock().expect("banners lock poisoned").insert(ip, banner);
        }
    }
}

/// Direct DDMS M-SEARCH aimed at one device — used by the poller for topology
/// and by the live probe. Returns the raw banner text from that IP.
pub async fn ddms_probe(ip: &str, timeout: Duration) -> Option<String> {
    let want: Ipv4Addr = ip.parse().ok()?;
    let socket = msearch_socket(DDMS_ST, DDMS_PORT).await.ok()?;
    let mut buf = vec![0u8; UDP_BUF];
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let received = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return None,
            received = socket.recv_from(&mut buf) => received,
        };
        let (len, from) = received.ok()?;
        if reply_ip(from) == Some(want) {
            return Some(String::from_utf8_lossy(&buf[..len]).into_owned());
        }
    }
}

// ------------------------------------------------------------------- SSDP --

async fn collect_ssdp(run: &Arc<ScanRun>) {
    let socket = match msearch_socket(SSDP_ST, SSDP_PORT).await {
        Ok(socket) => socket,
        Err(e) => return log::warn!("SSDP discovery: {e}"),
    };

    let mut buf = vec![0u8; UDP_BUF];
    let deadline = tokio::time::Instant::now() + SSDP_WINDOW;
    let mut seen: HashSet<Ipv4Addr> = HashSet::new();
    loop {
        let received = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            _ = run.token.cancelled() => break,
            received = socket.recv_from(&mut buf) => received,
        };
        let (len, from) = match received {
            Ok(v) => v,
            Err(e) => {
                log::debug!("SSDP receive failed: {e}");
                break;
            }
        };
        let text = String::from_utf8_lossy(&buf[..len]).into_owned();
        let target = location_target(&text).or_else(|| reply_ip(from).map(|ip| (ip, UPNP_PORT, "/description.xml".to_string())));
        let Some((ip, port, path)) = target else { continue };
        if !seen.insert(ip) {
            continue;
        }
        // Fetch description.xml for the stable UDN uuid.
        if let Some(identity) = fetch_identity(ip, port, &path).await {
            run.identities.lock().expect("identities lock poisoned").insert(ip, identity);
        }
    }
}

/// The stable UPnP UDN uuid (and model/name) for one device, via
/// `:49494/description.xml`. Used by `add_device` to key a manually-added unit.
/// Returns `(uuid, model, friendly_name)`.
pub async fn upnp_identity(ip: Ipv4Addr) -> Option<(String, String, String)> {
    fetch_identity(ip, UPNP_PORT, "/description.xml")
        .await
        .map(|i| (i.uuid, i.model, i.friendly_name))
}

/// Fetch and parse `description.xml`.
async fn fetch_identity(ip: Ipv4Addr, port: u16, path: &str) -> Option<UpnpIdentity> {
    let xml = http_get(ip, port, path).await?;
    let uuid = xml_field(&xml, "UDN")
        .map(|u| u.trim().strip_prefix("uuid:").unwrap_or(&u).to_string())
        .unwrap_or_default();
    let identity = UpnpIdentity {
        uuid,
        model: xml_field(&xml, "modelName").unwrap_or_default(),
        friendly_name: xml_field(&xml, "friendlyName").unwrap_or_default(),
    };
    // Only useful if it gave us something to key on.
    (!identity.uuid.is_empty() || !identity.model.is_empty()).then_some(identity)
}

/// Minimal HTTP/1.0 GET — no dependency, LAN-only, closes on response end.
async fn http_get(ip: Ipv4Addr, port: u16, path: &str) -> Option<String> {
    let fut = async {
        let mut stream = TcpStream::connect(SocketAddr::from((ip, port))).await.ok()?;
        let request = format!("GET {path} HTTP/1.0\r\nHost: {ip}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.ok()?;
        let text = String::from_utf8_lossy(&raw).into_owned();
        // Split headers from body at the blank line.
        Some(text.split_once("\r\n\r\n").map(|(_, body)| body.to_string()).unwrap_or(text))
    };
    tokio::time::timeout(HTTP_TIMEOUT, fut).await.ok().flatten()
}

// ------------------------------------------------------------------ sweep --

async fn sweep(run: &Arc<ScanRun>, net: Ipv4Net) {
    let local = net::local_ipv4();
    // Skip hosts DDMS already confirmed and this machine.
    let already: HashSet<Ipv4Addr> =
        run.banners.lock().expect("banners lock poisoned").keys().copied().collect();
    let hosts: Vec<Ipv4Addr> = net
        .hosts()
        .filter(|h| Some(*h) != local && !already.contains(h))
        .collect();
    run.set_total(Phase::Sweep, hosts.len() as u32);

    let mut probes = Vec::new();
    for host in hosts {
        if run.token.is_cancelled() {
            break;
        }
        let Ok(permit) = run.permits.clone().acquire_owned().await else { break };
        let run = run.clone();
        probes.push(tauri::async_runtime::spawn(async move {
            let _permit: OwnedSemaphorePermit = permit;
            sweep_confirm(&run, host).await;
            run.scanned(Phase::Sweep);
        }));
    }
    for probe in probes {
        let _ = probe.await;
    }
}

/// Confirm a swept host by TLS + `DevInfo(92)`.
async fn sweep_confirm(run: &Arc<ScanRun>, host: Ipv4Addr) {
    if run.token.is_cancelled() {
        return;
    }
    // Cheap gate: is :7777 even open? Avoids a TLS attempt on every dead host.
    let addr = SocketAddr::from((host, LUCI_PORT));
    match tokio::time::timeout(SWEEP_CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(_)) => {}
        _ => return,
    }

    let ip = host.to_string();
    let confirmed = tokio::time::timeout(SWEEP_TLS_TIMEOUT, async {
        let (client, _events) = LuciClient::connect(&ip).await.ok()?;
        let payload = client.read(MessageBox::DevInfo).await.ok()?;
        DevInfo::parse(&payload).map(|info| (client, info))
    })
    .await;

    let Ok(Some((client, info))) = confirmed else { return };
    // A friendly name, best-effort.
    let name = client.read(MessageBox::DevName).await.unwrap_or_default().trim().to_string();
    let identity = run.identities.lock().expect("identities lock poisoned").get(&host).cloned().unwrap_or_default();
    let usn = info.macaddress.eth0.clone();
    let candidate = DeviceCandidate {
        already_added: already_added(&identity.uuid, &usn, &ip, &run.saved),
        uuid: identity.uuid,
        usn,
        name,
        model: identity.model,
        firmware: info.versioninfo.devicefwversion,
        net_mode: None,
        wifi_band: None,
        ip,
    };
    run.record_candidate(candidate);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(uuid: &str, usn: &str, ip: &str) -> SavedDevice {
        SavedDevice { uuid: uuid.into(), usn: usn.into(), ip: ip.into(), ..SavedDevice::default() }
    }

    #[test]
    fn cidr_accepts_a_plain_network() {
        let net = parse_cidr("192.168.10.0/24").expect("a /24 must parse");
        assert_eq!(net.to_string(), "192.168.10.0/24");
        assert_eq!(net.hosts().count(), 254);
    }

    #[test]
    fn cidr_truncates_host_bits() {
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
        assert_eq!(sweep_cidr(&requested, Some("192.168.0.0/24")).as_deref(), Some("10.1.2.0/24"));

        let blank = ScanOptions { sweep: true, cidr: Some("   ".into()) };
        assert_eq!(
            sweep_cidr(&blank, Some("192.168.0.0/24")).as_deref(),
            Some("192.168.0.0/24"),
            "a blank input must not shadow the saved setting"
        );

        let auto = sweep_cidr(&ScanOptions::default(), None);
        if let Some(cidr) = auto {
            assert!(parse_cidr(&cidr).is_ok(), "auto-detected {cidr} must be sweepable");
        }
    }

    #[test]
    fn location_target_parses_ip_port_path() {
        let reply = "HTTP/1.1 200 OK\r\nLOCATION: http://192.168.10.104:49494/description.xml\r\n\r\n";
        assert_eq!(
            location_target(reply),
            Some((Ipv4Addr::new(192, 168, 10, 104), 49494, "/description.xml".to_string()))
        );
        // Header case-insensitive, default port when omitted.
        assert_eq!(
            location_target("location: http://10.0.0.5/desc.xml\r\n"),
            Some((Ipv4Addr::new(10, 0, 0, 5), UPNP_PORT, "/desc.xml".to_string()))
        );
        // Not an IPv4 literal → None (falls back to sender at the call site).
        assert_eq!(location_target("LOCATION: http://host.local:49494/d.xml\r\n"), None);
    }

    #[test]
    fn xml_field_extracts_the_udn() {
        let xml = "<root><device><UDN>uuid:afcea3b1-ae97-4c5a-9c2e-e2328542154a</UDN><modelName>LP10</modelName></device></root>";
        assert_eq!(xml_field(xml, "UDN").as_deref(), Some("uuid:afcea3b1-ae97-4c5a-9c2e-e2328542154a"));
        assert_eq!(xml_field(xml, "modelName").as_deref(), Some("LP10"));
        assert_eq!(xml_field(xml, "missing"), None);
    }

    #[test]
    fn candidate_key_prefers_uuid_then_usn_then_ip() {
        assert_eq!(candidate_key("U", "M", "1.2.3.4"), candidate_key("U", "X", "9.9.9.9"));
        assert_eq!(candidate_key("", "M", "1.2.3.4"), candidate_key("", "m", "9.9.9.9"), "usn is case-insensitive");
        assert_ne!(candidate_key("", "", "1.2.3.4"), candidate_key("", "", "9.9.9.9"));
    }

    #[test]
    fn already_added_matches_on_uuid_usn_or_ip() {
        let list = [saved("U1", "AA:BB", "192.168.1.4")];
        assert!(already_added("U1", "", "9.9.9.9", &list), "uuid match across a new IP");
        assert!(already_added("", "aa:bb", "9.9.9.9", &list), "usn match, case-insensitive");
        assert!(already_added("", "", "192.168.1.4", &list), "ip fallback");
        assert!(!already_added("U2", "CC:DD", "9.9.9.9", &list), "a different unit is new");
    }
}
