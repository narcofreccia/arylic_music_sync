//! Typed parses of Luci payloads, the DDMS discovery banner, and the app-facing
//! `DeviceSnapshot`.
//!
//! Everything here is deliberately tolerant: Luci scalars are bare ASCII
//! (`"30"`, `"UNMUTE"`), `DevInfo` is JSON, and the DDMS banner is CRLF
//! `KEY:VALUE` text — an unexpected field is ignored, never fatal.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// ------------------------------------------------------------- DevInfo (JSON) --

/// `DevInfo(92)` — device identity, MACs and firmware. The `eth0`/`wlan0` split
/// is the wired-vs-Wi-Fi signal the UI surfaces.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct DevInfo {
    pub macaddress: MacAddresses,
    pub serialnumber: SerialNumber,
    pub versioninfo: VersionInfo,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct MacAddresses {
    pub bt: String,
    pub eth0: String,
    pub wlan0: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SerialNumber {
    pub device_serialnumber: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct VersionInfo {
    pub devicefwversion: String,
    pub mcuversion: String,
}

impl DevInfo {
    /// Parse the JSON payload, tolerating unknown fields.
    pub fn parse(payload: &str) -> Option<Self> {
        serde_json::from_str(payload.trim()).ok()
    }
}

// ------------------------------------------------------------- scalar helpers --

/// `VOLUME(64)` → 0..=100.
pub fn parse_volume(payload: &str) -> Option<u8> {
    payload.trim().parse().ok()
}

/// `Mute_Unmute(63)` → `"MUTE"` = true, anything else (`"UNMUTE"`) = false.
pub fn parse_mute(payload: &str) -> bool {
    payload.trim().eq_ignore_ascii_case("MUTE")
}

/// `PLAY_STATE(51)` → the raw integer (`0` = stopped, `1` = playing, …).
pub fn parse_play_state(payload: &str) -> Option<i32> {
    payload.trim().parse().ok()
}

/// `CURRSOURCE(50)` → the raw source integer.
pub fn parse_source(payload: &str) -> Option<i32> {
    payload.trim().parse().ok()
}

/// `CURRSOURCE(50)`/`PlayBackSource(10)` integer → a human label.
///
/// Only two codes are field-verified on LP10 firmware `AR241CE_9243.16.2`:
/// `0` = idle (the wired main unit at rest) and `4` = an active audio stream
/// (the garden unit while playing). The exact service name behind each other
/// code is **not** confirmed on this hardware, so anything else degrades to
/// `"Source N"` rather than guessing (docs/firmware-notes.md).
pub fn source_label(source: i32) -> String {
    match source {
        0 => "Idle".to_string(),
        4 => "Streaming".to_string(),
        n => format!("Source {n}"),
    }
}

/// Whether a source is one the transport controls (play/pause/next/prev) can
/// meaningfully act on. `0` (idle) is not; an active stream is. Unknown codes
/// are treated as controllable so a real source is never left without controls.
pub fn source_controllable(source: i32) -> bool {
    source != 0
}

// --------------------------------------------------------------- DDMS banner --

/// A parsed DDMS M-SEARCH banner (CRLF `KEY:VALUE` lines). Keys are uppercased so
/// lookups are case-insensitive; the raw map is kept for the debug pane.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DdmsBanner {
    pub fields: BTreeMap<String, String>,
}

impl DdmsBanner {
    /// Parse a banner. Lines without a colon (the `HTTP/1.1 200 OK` status line,
    /// blank lines) are skipped.
    pub fn parse(text: &str) -> Self {
        let mut fields = BTreeMap::new();
        for line in text.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_ascii_uppercase();
                let value = value.trim().to_string();
                if !key.is_empty() {
                    fields.insert(key, value);
                }
            }
        }
        Self { fields }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(&key.to_ascii_uppercase()).map(String::as_str).filter(|v| !v.is_empty())
    }

    pub fn device_name(&self) -> Option<&str> {
        self.get("DEVICENAME")
    }

    pub fn state(&self) -> Option<&str> {
        self.get("STATE")
    }

    pub fn usn(&self) -> Option<&str> {
        self.get("USN")
    }

    pub fn port(&self) -> Option<u16> {
        self.get("PORT").and_then(|p| p.parse().ok())
    }

    pub fn firmware(&self) -> Option<&str> {
        self.get("FWVERSION")
    }

    pub fn model(&self) -> Option<&str> {
        self.get("CAST_MODEL")
    }

    /// `NETMODE` → the app's `"ethernet"` / `"wifi"` distinction.
    pub fn net_mode(&self) -> Option<NetMode> {
        match self.get("NETMODE") {
            Some(m) if m.eq_ignore_ascii_case("ETH0") || m.eq_ignore_ascii_case("ETH") => Some(NetMode::Ethernet),
            Some(m) if m.to_ascii_uppercase().starts_with("WLAN") || m.eq_ignore_ascii_case("WIFI") => Some(NetMode::Wifi),
            _ => None,
        }
    }

    /// `WIFIBAND` (`ETH` | `2G` | `5G`), verbatim.
    pub fn wifi_band(&self) -> Option<&str> {
        self.get("WIFIBAND")
    }

    /// Best-effort group role from the `State` field. `S`/empty = standalone;
    /// a leading `M` reads as master; anything else as slave. The exact grouped
    /// letters are an R2 live-derivation task, so this stays conservative.
    pub fn role(&self) -> Role {
        match self.state() {
            None => Role::Solo,
            Some(s) => {
                let s = s.trim().to_ascii_uppercase();
                if s.is_empty() || s == "S" {
                    Role::Solo
                } else if s.starts_with('M') {
                    Role::Master
                } else {
                    Role::Slave
                }
            }
        }
    }
}

// ---------------------------------------------------------------- app models --

/// Wired vs Wi-Fi, from `DevInfo` MACs cross-checked with the DDMS banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetMode {
    Ethernet,
    Wifi,
}

/// Group role (R1 reads it; R2 will act on it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Solo,
    Master,
    Slave,
}

/// Now-playing metadata (best-effort from `TRACK_INFO`/`GETPLAYDURATION`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Total length in ms, when known.
    pub duration_ms: Option<u64>,
    /// Current position in ms (from `GETPLAYDURATION` pushes), when known.
    pub position_ms: Option<u64>,
}

impl Track {
    /// True when there is nothing worth showing.
    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
            && self.artist.is_empty()
            && self.album.is_empty()
            && self.duration_ms.is_none()
            && self.position_ms.is_none()
    }

    /// Parse the `TRACK_INFO(44)` payload. The exact schema is firmware-specific;
    /// we try JSON and pull common field spellings, ignoring the rest.
    pub fn parse_track_info(payload: &str) -> Track {
        let mut track = Track::default();
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(payload.trim()) {
            let pick = |keys: &[&str]| -> String {
                for k in keys {
                    if let Some(Value::String(s)) = map.get(*k) {
                        if !s.trim().is_empty() {
                            return s.trim().to_string();
                        }
                    }
                }
                String::new()
            };
            track.title = pick(&["title", "Title", "TITLE", "trackName"]);
            track.artist = pick(&["artist", "Artist", "ARTIST", "artistName"]);
            track.album = pick(&["album", "Album", "ALBUM", "albumName"]);
            for k in ["duration", "totlen", "totalTime", "durationMs"] {
                match map.get(k) {
                    Some(Value::Number(n)) => {
                        track.duration_ms = n.as_u64();
                        break;
                    }
                    Some(Value::String(s)) => {
                        if let Ok(v) = s.trim().parse() {
                            track.duration_ms = Some(v);
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        track
    }
}

/// The per-device state the frontend renders and the poller diffs. `PartialEq`
/// is what "emit only on change" is built on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshot {
    pub uuid: String,
    pub ip: String,
    /// Name reported by the device (DDMS `DeviceName` / `DevName`).
    pub name: String,
    /// Local override (R1 rename); `None` = use the device's own name.
    pub alias: Option<String>,
    /// Alias, else device name, else IP — what the UI prints.
    pub display_name: String,
    pub online: bool,
    /// `ethernet` | `wifi`, when known.
    pub net_mode: Option<NetMode>,
    /// `ETH` | `2G` | `5G`, verbatim.
    pub wifi_band: Option<String>,
    pub model: String,
    pub firmware: String,
    pub role: Role,
    /// DDMS zone id, when grouped (R2 fills this in fully).
    pub group_id: Option<String>,
    /// The master this device follows, when a slave.
    pub master_uuid: Option<String>,
    pub volume: Option<u8>,
    pub mute: bool,
    /// Raw `CURRSOURCE` integer.
    pub source: Option<i32>,
    /// Human label for `source` (`Idle` / `Streaming` / `Source N`).
    pub source_label: Option<String>,
    /// Raw `PLAY_STATE` integer (`0`/`1`).
    pub play_state: Option<i32>,
    pub track: Option<Track>,
    /// Unix ms of the last successful contact.
    pub last_seen: Option<i64>,
    /// Verbatim raw payloads, for the debug pane.
    pub raw: Map<String, Value>,
}

impl DeviceSnapshot {
    /// The offline placeholder for a saved device we haven't reached yet.
    pub fn offline(uuid: &str, ip: &str, alias: Option<String>, last_seen: Option<i64>) -> Self {
        Self {
            uuid: uuid.to_string(),
            ip: ip.to_string(),
            name: String::new(),
            display_name: display_name(alias.as_deref(), "", ip),
            alias,
            online: false,
            net_mode: None,
            wifi_band: None,
            model: String::new(),
            firmware: String::new(),
            role: Role::Solo,
            group_id: None,
            master_uuid: None,
            volume: None,
            mute: false,
            source: None,
            source_label: None,
            play_state: None,
            track: None,
            last_seen,
            raw: Map::new(),
        }
    }

    /// Recompute `display_name` after `alias`/`name` change.
    pub fn refresh_display_name(&mut self) {
        self.display_name = display_name(self.alias.as_deref(), &self.name, &self.ip);
    }

    /// Mark a previously-known snapshot unreachable, keeping identity fields so
    /// the card stays recognisable while greyed out.
    pub fn mark_offline(&mut self) {
        self.online = false;
        self.volume = None;
        self.mute = false;
        self.source = None;
        self.source_label = None;
        self.play_state = None;
        self.track = None;
        self.role = Role::Solo;
        self.group_id = None;
        self.master_uuid = None;
    }
}

/// What the UI prints: alias, else device name, else IP.
pub fn display_name(alias: Option<&str>, device_name: &str, ip: &str) -> String {
    match alias.map(str::trim) {
        Some(a) if !a.is_empty() => a.to_string(),
        _ if !device_name.trim().is_empty() => device_name.trim().to_string(),
        _ => ip.to_string(),
    }
}

/// The detail view — a snapshot plus its verbatim raw payloads.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDetail {
    pub snapshot: DeviceSnapshot,
    /// Raw Luci/DDMS payloads keyed by source (`devInfo`, `volume`, `ddms`, …).
    pub raw: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devinfo_parses_the_live_payload() {
        let json = r#"{"macaddress":{"bt":"F4:AB:5C:FC:A8:2F","eth0":"00:E0:3A:00:0A:8A","wlan0":"D8:F7:10:71:86:28"},"serialnumber":{"device_serialnumber":"RKARYLLP102625004937"},"versioninfo":{"devicefwversion":"AR241CE_9243.16.2","mcuversion":"16"}}"#;
        let info = DevInfo::parse(json).expect("must parse");
        assert_eq!(info.macaddress.eth0, "00:E0:3A:00:0A:8A");
        assert_eq!(info.macaddress.wlan0, "D8:F7:10:71:86:28");
        assert_eq!(info.serialnumber.device_serialnumber, "RKARYLLP102625004937");
        assert_eq!(info.versioninfo.devicefwversion, "AR241CE_9243.16.2");
        assert_eq!(info.versioninfo.mcuversion, "16");
    }

    #[test]
    fn devinfo_tolerates_unknown_and_missing_fields() {
        let info = DevInfo::parse(r#"{"macaddress":{"eth0":"AA"},"extra":"ignored"}"#).expect("parse");
        assert_eq!(info.macaddress.eth0, "AA");
        assert_eq!(info.macaddress.wlan0, "");
        assert_eq!(info.versioninfo.devicefwversion, "");
        assert!(DevInfo::parse("not json").is_none());
    }

    #[test]
    fn scalar_helpers() {
        assert_eq!(parse_volume("30"), Some(30));
        assert_eq!(parse_volume(" 25 "), Some(25));
        assert_eq!(parse_volume("n/a"), None);
        assert!(parse_mute("MUTE"));
        assert!(parse_mute("mute"));
        assert!(!parse_mute("UNMUTE"));
        assert_eq!(parse_play_state("1"), Some(1));
        assert_eq!(parse_source("10"), Some(10));
    }

    #[test]
    fn source_label_maps_verified_codes_and_degrades() {
        assert_eq!(source_label(0), "Idle");
        assert_eq!(source_label(4), "Streaming");
        assert_eq!(source_label(2), "Source 2");
        assert_eq!(source_label(99), "Source 99");
        assert!(!source_controllable(0));
        assert!(source_controllable(4));
        assert!(source_controllable(7));
    }

    #[test]
    fn ddms_banner_parses_and_classifies() {
        let banner = "HTTP/1.1 200 OK\r\n\
            DeviceName:Lofficina-main\r\n\
            State:S\r\n\
            NETMODE:ETH0\r\n\
            WIFIBAND:ETH\r\n\
            PORT:7777\r\n\
            TCPPORT:2020\r\n\
            MRAMode:DDMS\r\n\
            USN:00:E0:3A:00:0A:8A\r\n\
            FWVERSION:AR241CE_9243\r\n\
            CAST_MODEL:LP10\r\n";
        let b = DdmsBanner::parse(banner);
        assert_eq!(b.device_name(), Some("Lofficina-main"));
        assert_eq!(b.net_mode(), Some(NetMode::Ethernet));
        assert_eq!(b.wifi_band(), Some("ETH"));
        assert_eq!(b.port(), Some(7777));
        assert_eq!(b.usn(), Some("00:E0:3A:00:0A:8A"));
        assert_eq!(b.model(), Some("LP10"));
        assert_eq!(b.role(), Role::Solo);
    }

    #[test]
    fn ddms_wlan_is_wifi_and_grouped_state_is_not_solo() {
        let b = DdmsBanner::parse("DeviceName:Garden\r\nState:M\r\nNETMODE:WLAN\r\nWIFIBAND:2G\r\n");
        assert_eq!(b.net_mode(), Some(NetMode::Wifi));
        assert_eq!(b.wifi_band(), Some("2G"));
        assert_eq!(b.role(), Role::Master);

        let empty = DdmsBanner::parse("NETMODE:WLAN\r\n");
        assert_eq!(empty.role(), Role::Solo, "no State reads as solo");
    }

    #[test]
    fn track_info_parse_is_tolerant() {
        let t = Track::parse_track_info(r#"{"title":"Song","artist":"Band","duration":210000}"#);
        assert_eq!(t.title, "Song");
        assert_eq!(t.artist, "Band");
        assert_eq!(t.duration_ms, Some(210000));
        assert!(Track::parse_track_info("garbage").is_empty());
    }

    #[test]
    fn display_name_prefers_alias_then_name_then_ip() {
        assert_eq!(display_name(Some("Cucina"), "LP10", "192.168.1.5"), "Cucina");
        assert_eq!(display_name(Some("  "), "LP10", "192.168.1.5"), "LP10");
        assert_eq!(display_name(None, "", "192.168.1.5"), "192.168.1.5");
    }
}
