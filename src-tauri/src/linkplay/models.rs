//! Typed models for the Linkplay HTTP API (NFR-6) — and the DTOs the frontend
//! actually renders.
//!
//! The API is undocumented and varies across firmware generations (brief §9), so
//! every wire struct here is deliberately tolerant:
//!
//! * every field is `#[serde(default)]` — a missing key is never an error;
//! * unknown keys are captured in `extra` rather than dropped, which is what
//!   feeds the debug pane in the device detail view (and the FR-23 spike);
//! * numbers are parsed leniently, because Linkplay returns `"vol": 42` on one
//!   firmware and `"vol": "42"` on the next.
//!
//! The `*Info`/`Snapshot` types below are the app's own shape and serialize as
//! camelCase for the TypeScript side.

use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use super::hexstr;

// ------------------------------------------------------------ deserializers --

/// Parse a number that may arrive as a JSON number, a quoted string, a bool, or
/// not at all. Anything unparseable degrades to `T::default()` rather than
/// failing the whole response — one odd field must not cost us the device.
pub fn de_num<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr + Default,
    <T as FromStr>::Err: Display,
{
    Ok(de_num_opt(d)?.unwrap_or_default())
}

/// Like [`de_num`] but keeps "absent/blank" distinct from zero — RSSI 0 and "no
/// RSSI reported" are genuinely different things to show in the UI.
pub fn de_num_opt<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr + Default,
    <T as FromStr>::Err: Display,
{
    let value = Value::deserialize(d)?;
    Ok(parse_lenient(&value))
}

/// Booleans arrive as `1`/`0`, `"1"`/`"0"`, or occasionally real booleans.
pub fn de_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let value = Value::deserialize(d)?;
    Ok(match &value {
        Value::Bool(b) => *b,
        Value::String(s) if s.eq_ignore_ascii_case("true") => true,
        Value::String(s) if s.eq_ignore_ascii_case("false") => false,
        other => parse_lenient::<i64>(other).unwrap_or(0) != 0,
    })
}

fn parse_lenient<T: FromStr>(value: &Value) -> Option<T> {
    match value {
        Value::Number(n) => {
            let i = n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64))?;
            i.to_string().parse().ok()
        }
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            // "42" first; "42.0" via the float path (some firmwares do that).
            s.parse().ok().or_else(|| {
                let i = s.parse::<f64>().ok()?.trunc() as i64;
                i.to_string().parse().ok()
            })
        }
        Value::Bool(b) => if *b { "1" } else { "0" }.parse().ok(),
        _ => None,
    }
}

/// Blank strings mean "unset" on this API far more often than they mean "".
fn blank_to_none(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() || s == "0.0.0.0" {
        None
    } else {
        Some(s.to_string())
    }
}

// ------------------------------------------------------------ wire: getStatusEx --

/// `getStatusEx` — device identity, firmware and group role.
///
/// Field names mirror the wire exactly (including Linkplay's inconsistent
/// capitalisation) so the mapping stays obvious against a raw dump.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatusEx {
    #[serde(default)]
    pub uuid: String,
    /// Left un-decoded on purpose: whether names are ever hex-encoded is an open
    /// question for the spike (firmware-notes §6), and a wrong guess renames the
    /// user's speaker in the UI.
    #[serde(rename = "DeviceName", default)]
    pub device_name: String,
    #[serde(rename = "GroupName", default)]
    pub group_name: String,
    #[serde(default)]
    pub firmware: String,
    #[serde(default)]
    pub hardware: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub mcu_ver: String,
    #[serde(rename = "RSSI", default, deserialize_with = "de_num_opt")]
    pub rssi: Option<i32>,
    #[serde(default)]
    pub ssid: String,
    #[serde(default)]
    pub essid: String,
    /// `1` while this device follows a master. The primary role signal.
    #[serde(default, deserialize_with = "de_num")]
    pub group: u8,
    #[serde(default)]
    pub master_uuid: String,
    #[serde(default)]
    pub master_ip: String,
    /// Wired address (empty when the unit is on Wi-Fi only).
    #[serde(default)]
    pub eth2: String,
    /// Wi-Fi client address.
    #[serde(default)]
    pub apcli0: String,
    #[serde(default)]
    pub upnp_uuid: String,

    /// Everything this firmware sends that we don't model yet — surfaced raw in
    /// the debug pane so the spike can be run from the app itself.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

// -------------------------------------------------------- wire: getPlayerStatus --

/// `getPlayerStatus` — transport state, volume and the current track.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayerStatus {
    /// `play` | `stop` | `pause` | `load`.
    #[serde(default)]
    pub status: String,
    /// Input/source code — see [`source_label`].
    #[serde(default, deserialize_with = "de_num")]
    pub mode: u32,
    #[serde(default, deserialize_with = "de_num")]
    pub vol: u8,
    #[serde(default, deserialize_with = "de_bool")]
    pub mute: bool,
    /// Position/length in ms.
    #[serde(default, deserialize_with = "de_num")]
    pub curpos: u64,
    #[serde(default, deserialize_with = "de_num")]
    pub totlen: u64,
    #[serde(rename = "Title", default, deserialize_with = "hexstr::de")]
    pub title: String,
    #[serde(rename = "Artist", default, deserialize_with = "hexstr::de")]
    pub artist: String,
    #[serde(rename = "Album", default, deserialize_with = "hexstr::de")]
    pub album: String,

    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Human label for a `getPlayerStatus.mode` (FR-19). Unknown codes are shown as
/// `mode <n>` rather than swallowed — a firmware we haven't seen should still
/// tell the user *something*, and the number is the bug report.
pub fn source_label(mode: u32) -> String {
    match mode {
        0 => "idle",
        1 => "airplay",
        2 => "dlna",
        10 => "network",
        11 => "usb",
        31 => "spotify",
        40 => "line-in",
        41 => "bluetooth",
        43 => "optical",
        47 => "line-in2",
        51 => "usb-dac",
        99 => "follower",
        n => return format!("mode {n}"),
    }
    .to_string()
}

// --------------------------------------------------------- wire: getSlaveList --

/// `multiroom:getSlaveList` — the master's view of its group.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlaveList {
    #[serde(default, deserialize_with = "de_num")]
    pub slaves: u32,
    #[serde(default)]
    pub slave_list: Vec<SlaveEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlaveEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, deserialize_with = "de_num")]
    pub channel: i32,
    #[serde(default, deserialize_with = "de_num")]
    pub volume: u8,
    #[serde(default, deserialize_with = "de_bool")]
    pub mute: bool,

    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

// ------------------------------------------------------------------- app DTOs --

/// Group role (FR-9/FR-13), derived from `getStatusEx` + `getSlaveList`.
///
/// Serialized as `{ kind: "master", slaveUuids: [...] }` so the frontend can
/// switch on one discriminant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DeviceRole {
    #[serde(rename_all = "camelCase")]
    Master { slave_uuids: Vec<String> },
    #[serde(rename_all = "camelCase")]
    Slave {
        master_uuid: Option<String>,
        master_ip: Option<String>,
    },
    Solo,
}

/// Derive the group role.
///
/// Order matters: "am I following someone" beats "does someone follow me",
/// because a firmware that has just been kicked can briefly report a stale slave
/// list while `group`/`master_ip` are already correct.
///
/// `player_mode` is the cross-check: mode 99 ("follower") is a slave signal on
/// firmwares that leave `group` at 0 — but it can only tell us *that* we follow,
/// not whom, so it is the last resort.
pub fn derive_role(status: &StatusEx, slaves: Option<&SlaveList>, player_mode: Option<u32>) -> DeviceRole {
    let master_ip = blank_to_none(&status.master_ip);
    let master_uuid = blank_to_none(&status.master_uuid);

    if status.group == 1 || master_ip.is_some() {
        return DeviceRole::Slave { master_uuid, master_ip };
    }
    if let Some(list) = slaves {
        if list.slaves > 0 || !list.slave_list.is_empty() {
            let slave_uuids = list
                .slave_list
                .iter()
                .map(|s| if s.uuid.is_empty() { s.ip.clone() } else { s.uuid.clone() })
                .filter(|id| !id.is_empty())
                .collect();
            return DeviceRole::Master { slave_uuids };
        }
    }
    if player_mode == Some(99) {
        return DeviceRole::Slave { master_uuid, master_ip };
    }
    DeviceRole::Solo
}

/// Playback state as the UI wants it (FR-18/FR-19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerInfo {
    pub status: String,
    /// Raw `mode` code, kept for the debug pane.
    pub mode: u32,
    /// Label for `mode` — `"spotify"`, `"line-in"`, `"mode 77"`…
    pub source: String,
    pub vol: u8,
    pub mute: bool,
    pub curpos: u64,
    pub totlen: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
}

impl From<&PlayerStatus> for PlayerInfo {
    fn from(p: &PlayerStatus) -> Self {
        Self {
            status: p.status.clone(),
            mode: p.mode,
            source: source_label(p.mode),
            vol: p.vol,
            mute: p.mute,
            curpos: p.curpos,
            totlen: p.totlen,
            title: p.title.clone(),
            artist: p.artist.clone(),
            album: p.album.clone(),
        }
    }
}

/// A group member as listed by the master.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlaveInfo {
    pub uuid: String,
    pub name: String,
    pub ip: String,
    pub volume: u8,
    pub mute: bool,
}

impl From<&SlaveEntry> for SlaveInfo {
    fn from(s: &SlaveEntry) -> Self {
        Self {
            uuid: s.uuid.clone(),
            name: s.name.clone(),
            ip: s.ip.clone(),
            volume: s.volume,
            mute: s.mute,
        }
    }
}

/// The per-device state the frontend renders and the poller diffs.
///
/// `PartialEq` is what "only emit on change" is built on (see `poller`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshot {
    pub uuid: String,
    pub ip: String,
    /// Name reported by the device itself.
    pub name: String,
    /// Local override (FR-7); `None` means "use the device's own name".
    pub alias: Option<String>,
    /// What the UI should print — alias, else device name, else the IP.
    pub display_name: String,
    pub online: bool,
    pub role: DeviceRole,
    pub group_name: String,
    pub firmware: String,
    pub hardware: String,
    pub project: String,
    pub mcu_ver: String,
    pub rssi: Option<i32>,
    pub ssid: String,
    /// Absent while the device is idle, a follower, or offline.
    pub player: Option<PlayerInfo>,
    pub slaves: Vec<SlaveInfo>,
    /// Unix ms of the last successful poll; `None` if we never reached it.
    pub last_seen: Option<i64>,
}

impl DeviceSnapshot {
    /// The offline placeholder for a saved device we haven't reached yet — what
    /// `list_devices` returns on a cold start so the list renders immediately.
    pub fn offline(uuid: &str, ip: &str, alias: Option<String>, last_seen: Option<i64>) -> Self {
        Self {
            uuid: uuid.to_string(),
            ip: ip.to_string(),
            name: String::new(),
            display_name: display_name(alias.as_deref(), "", ip),
            alias,
            online: false,
            role: DeviceRole::Solo,
            group_name: String::new(),
            firmware: String::new(),
            hardware: String::new(),
            project: String::new(),
            mcu_ver: String::new(),
            rssi: None,
            ssid: String::new(),
            player: None,
            slaves: Vec::new(),
            last_seen,
        }
    }

    /// Build a live snapshot from a poll round.
    pub fn build(
        ip: &str,
        alias: Option<String>,
        status: &StatusEx,
        player: Option<&PlayerStatus>,
        slaves: Option<&SlaveList>,
        last_seen: i64,
    ) -> Self {
        let role = derive_role(status, slaves, player.map(|p| p.mode));
        Self {
            uuid: status.uuid.clone(),
            ip: ip.to_string(),
            name: status.device_name.clone(),
            display_name: display_name(alias.as_deref(), &status.device_name, ip),
            alias,
            online: true,
            role,
            group_name: status.group_name.clone(),
            firmware: status.firmware.clone(),
            hardware: status.hardware.clone(),
            project: status.project.clone(),
            mcu_ver: status.mcu_ver.clone(),
            rssi: status.rssi,
            ssid: if status.ssid.is_empty() { status.essid.clone() } else { status.ssid.clone() },
            player: player.map(PlayerInfo::from),
            slaves: slaves.map(|l| l.slave_list.iter().map(SlaveInfo::from).collect()).unwrap_or_default(),
            last_seen: Some(last_seen),
        }
    }

    /// Mark a previously-known snapshot as unreachable, keeping the identity
    /// fields so the card doesn't blank out when a device drops off.
    pub fn mark_offline(&mut self) {
        self.online = false;
        self.player = None;
        self.slaves.clear();
        self.role = DeviceRole::Solo;
    }
}

fn display_name(alias: Option<&str>, device_name: &str, ip: &str) -> String {
    match alias.map(str::trim) {
        Some(a) if !a.is_empty() => a.to_string(),
        _ if !device_name.trim().is_empty() => device_name.trim().to_string(),
        _ => ip.to_string(),
    }
}

/// Everything the detail view shows (FR-9), including the raw unmodelled fields.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDetail {
    pub snapshot: DeviceSnapshot,
    /// Unmodelled `getStatusEx` keys — the debug pane.
    pub extra: Map<String, Value>,
    /// Unmodelled `getPlayerStatus` keys.
    pub player_extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_from(json: &str) -> StatusEx {
        serde_json::from_str(json).expect("StatusEx must tolerate this payload")
    }

    #[test]
    fn de_num_accepts_strings_and_numbers() {
        let a = status_from(r#"{"group":"1","RSSI":"-52"}"#);
        assert_eq!(a.group, 1);
        assert_eq!(a.rssi, Some(-52));

        let b = status_from(r#"{"group":1,"RSSI":-52}"#);
        assert_eq!(b.group, 1);
        assert_eq!(b.rssi, Some(-52));
    }

    #[test]
    fn de_num_degrades_instead_of_failing() {
        let s = status_from(r#"{"group":"","RSSI":"n/a"}"#);
        assert_eq!(s.group, 0, "blank must fall back to the default");
        assert_eq!(s.rssi, None, "unparseable optional must be None, not 0");
    }

    #[test]
    fn de_bool_reads_zero_one_strings() {
        let p: PlayerStatus = serde_json::from_str(r#"{"mute":"1","vol":"36"}"#).unwrap();
        assert!(p.mute);
        assert_eq!(p.vol, 36);
        let p2: PlayerStatus = serde_json::from_str(r#"{"mute":0}"#).unwrap();
        assert!(!p2.mute);
    }

    #[test]
    fn unknown_fields_land_in_extra() {
        let s = status_from(r#"{"uuid":"FF31","branch":"stable","plm_support":"0"}"#);
        assert_eq!(s.uuid, "FF31");
        assert!(s.extra.contains_key("branch"));
        assert!(s.extra.contains_key("plm_support"));
        assert!(!s.extra.contains_key("uuid"), "modelled keys must not be duplicated");
    }

    #[test]
    fn player_status_decodes_hex_metadata() {
        let raw = format!(
            r#"{{"status":"play","mode":"31","Title":"{}","Artist":"Radiohead"}}"#,
            hex::encode("Everything In Its Right Place")
        );
        let p: PlayerStatus = serde_json::from_str(&raw).unwrap();
        assert_eq!(p.title, "Everything In Its Right Place");
        assert_eq!(p.artist, "Radiohead", "plain text must pass through");
        assert_eq!(source_label(p.mode), "spotify");
    }

    #[test]
    fn source_label_names_known_modes_and_numbers_the_rest() {
        assert_eq!(source_label(0), "idle");
        assert_eq!(source_label(41), "bluetooth");
        assert_eq!(source_label(99), "follower");
        assert_eq!(source_label(77), "mode 77");
    }

    // ------------------------------------------------------ role derivation --

    #[test]
    fn solo_device_is_solo() {
        let s = status_from(r#"{"uuid":"A","group":"0","master_ip":""}"#);
        let empty = SlaveList::default();
        assert_eq!(derive_role(&s, Some(&empty), Some(10)), DeviceRole::Solo);
    }

    #[test]
    fn slave_detected_from_group_flag() {
        let s = status_from(r#"{"uuid":"B","group":"1","master_uuid":"A","master_ip":"192.168.1.10"}"#);
        assert_eq!(
            derive_role(&s, None, None),
            DeviceRole::Slave {
                master_uuid: Some("A".into()),
                master_ip: Some("192.168.1.10".into()),
            }
        );
    }

    #[test]
    fn slave_detected_from_master_ip_alone() {
        let s = status_from(r#"{"uuid":"B","group":"0","master_ip":"192.168.1.10"}"#);
        assert!(matches!(derive_role(&s, None, None), DeviceRole::Slave { .. }));
        // A zeroed master_ip is "unset", not an address.
        let s0 = status_from(r#"{"uuid":"B","group":"0","master_ip":"0.0.0.0"}"#);
        assert_eq!(derive_role(&s0, None, None), DeviceRole::Solo);
    }

    #[test]
    fn master_detected_from_slave_list() {
        let s = status_from(r#"{"uuid":"A","group":"0"}"#);
        let list: SlaveList = serde_json::from_str(
            r#"{"slaves":"2","slave_list":[
                 {"name":"Kitchen","uuid":"B","ip":"192.168.1.11","volume":"30"},
                 {"name":"Bath","uuid":"C","ip":"192.168.1.12","volume":22}]}"#,
        )
        .unwrap();
        assert_eq!(
            derive_role(&s, Some(&list), Some(10)),
            DeviceRole::Master { slave_uuids: vec!["B".into(), "C".into()] }
        );
    }

    #[test]
    fn following_beats_a_stale_slave_list() {
        // Just kicked: still lists a slave, but already reports a master.
        let s = status_from(r#"{"uuid":"B","group":"1","master_ip":"192.168.1.10"}"#);
        let list: SlaveList =
            serde_json::from_str(r#"{"slaves":1,"slave_list":[{"uuid":"C","ip":"192.168.1.12"}]}"#).unwrap();
        assert!(matches!(derive_role(&s, Some(&list), None), DeviceRole::Slave { .. }));
    }

    #[test]
    fn player_mode_99_is_the_fallback_slave_signal() {
        let s = status_from(r#"{"uuid":"B","group":"0"}"#);
        assert_eq!(
            derive_role(&s, Some(&SlaveList::default()), Some(99)),
            DeviceRole::Slave { master_uuid: None, master_ip: None }
        );
    }

    #[test]
    fn slave_list_entries_fall_back_to_ip_when_uuid_is_missing() {
        let s = status_from(r#"{"uuid":"A"}"#);
        let list: SlaveList =
            serde_json::from_str(r#"{"slaves":1,"slave_list":[{"name":"K","ip":"192.168.1.11"}]}"#).unwrap();
        assert_eq!(
            derive_role(&s, Some(&list), None),
            DeviceRole::Master { slave_uuids: vec!["192.168.1.11".into()] }
        );
    }

    #[test]
    fn display_name_prefers_alias_then_device_name_then_ip() {
        assert_eq!(display_name(Some("Cucina"), "LP10", "192.168.1.5"), "Cucina");
        assert_eq!(display_name(Some("  "), "LP10", "192.168.1.5"), "LP10");
        assert_eq!(display_name(None, "", "192.168.1.5"), "192.168.1.5");
    }
}
