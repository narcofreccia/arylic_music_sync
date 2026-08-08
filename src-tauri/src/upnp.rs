//! UPnP / DLNA fallback over the MediaRenderer on `<ip>:49494`.
//!
//! Two things Luci does not do well on LP10 firmware `AR241CE_9243.16.2`:
//!
//! * **Transport** (play/pause/next/prev/stop). Luci `PLAYCNTRL(40)` *acks* every
//!   payload (verbs and numeric codes alike) with status 1 but never changes
//!   `PLAY_STATE` on an idle unit — the same "accepted, no effect" signature the
//!   DDMS grouping verbs have (docs/firmware-notes.md §G). The standard UPnP
//!   `AVTransport` actions, by contrast, are the proven per-device control path
//!   (§A), so transport goes through SOAP here.
//! * **Now-playing metadata.** `TRACK_INFO(44)` does not answer at all on this
//!   firmware (it times out even while a source is active), whereas
//!   `GetPositionInfo` returns DIDL-Lite with title/artist/album + duration and
//!   a live position.
//!
//! A minimal hand-rolled SOAP client (raw HTTP/1.0, no dependency, LAN-only) —
//! the same lightweight approach `discovery::http_get` already uses.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::{AppError, AppResult};

/// The MediaRenderer control port (docs/firmware-notes.md §A).
const UPNP_PORT: u16 = 49494;
const TRANSPORT_PATH: &str = "/upnp/control/rendertransport1";
const TRANSPORT_SERVICE: &str = "urn:schemas-upnp-org:service:AVTransport:1";

/// A whole SOAP round trip (connect + request + response) must finish in this.
const SOAP_TIMEOUT: Duration = Duration::from_secs(4);

/// The transport verbs the UI exposes. `as_action` is the AVTransport action
/// name; `Play` additionally needs a `Speed` argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Play,
    Pause,
    Next,
    Previous,
    Stop,
}

impl Transport {
    /// Parse the frontend's lowercase verb.
    pub fn parse(cmd: &str) -> Option<Self> {
        match cmd.trim().to_ascii_lowercase().as_str() {
            "play" => Some(Self::Play),
            "pause" => Some(Self::Pause),
            "next" => Some(Self::Next),
            "prev" | "previous" => Some(Self::Previous),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Play => "Play",
            Self::Pause => "Pause",
            Self::Next => "Next",
            Self::Previous => "Previous",
            Self::Stop => "Stop",
        }
    }

    /// The action's arguments (besides `InstanceID`), as `(name, value)` pairs.
    fn extra_args(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Play => &[("Speed", "1")],
            _ => &[],
        }
    }
}

/// Now-playing pulled from `GetPositionInfo` (DIDL-Lite metadata + timings).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: Option<u64>,
    pub position_ms: Option<u64>,
}

impl NowPlaying {
    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
            && self.artist.is_empty()
            && self.album.is_empty()
            && self.duration_ms.is_none()
    }
}

/// Fire an AVTransport transport action. Returns once the device answers 200.
pub async fn transport(ip: &str, cmd: Transport) -> AppResult<()> {
    let addr = resolve(ip)?;
    let mut body = String::from("<InstanceID>0</InstanceID>");
    for (name, value) in cmd.extra_args() {
        body.push_str(&format!("<{name}>{value}</{name}>"));
    }
    soap(addr, TRANSPORT_PATH, TRANSPORT_SERVICE, cmd.action(), &body).await?;
    Ok(())
}

/// Read now-playing via `GetPositionInfo`. Absent metadata is not an error —
/// a stopped renderer answers with empty fields.
pub async fn now_playing(ip: &str) -> AppResult<NowPlaying> {
    let addr = resolve(ip)?;
    let xml = soap(
        addr,
        TRANSPORT_PATH,
        TRANSPORT_SERVICE,
        "GetPositionInfo",
        "<InstanceID>0</InstanceID>",
    )
    .await?;
    Ok(parse_position_info(&xml))
}

fn resolve(ip: &str) -> AppResult<SocketAddr> {
    let addr: Ipv4Addr = ip
        .parse()
        .map_err(|_| AppError::Device(format!("“{ip}” is not a valid IPv4 address.")))?;
    Ok(SocketAddr::from((addr, UPNP_PORT)))
}

/// One SOAP action over raw HTTP/1.0. Returns the response body on `200 OK`.
async fn soap(addr: SocketAddr, path: &str, service: &str, action: &str, inner: &str) -> AppResult<String> {
    let envelope = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body><u:{action} xmlns:u=\"{service}\">{inner}</u:{action}></s:Body></s:Envelope>"
    );
    let request = format!(
        "POST {path} HTTP/1.0\r\n\
Host: {host}\r\n\
Content-Type: text/xml; charset=\"utf-8\"\r\n\
Content-Length: {len}\r\n\
SOAPACTION: \"{service}#{action}\"\r\n\
Connection: close\r\n\r\n{envelope}",
        host = addr,
        len = envelope.len(),
    );

    let fut = async {
        let mut stream = TcpStream::connect(addr)
            .await
            .map_err(|e| AppError::Device(format!("{} is unreachable on the UPnP port: {e}", addr.ip())))?;
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| AppError::Device(format!("{}: UPnP write failed: {e}", addr.ip())))?;
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .await
            .map_err(|e| AppError::Device(format!("{}: UPnP read failed: {e}", addr.ip())))?;
        Ok::<Vec<u8>, AppError>(raw)
    };

    let raw = tokio::time::timeout(SOAP_TIMEOUT, fut)
        .await
        .map_err(|_| AppError::Device(format!("{} did not answer the UPnP request in time.", addr.ip())))??;

    let text = String::from_utf8_lossy(&raw).into_owned();
    let (headers, body) = text.split_once("\r\n\r\n").unwrap_or(("", &text));
    let status_ok = headers.lines().next().is_some_and(|l| l.contains(" 200"));
    if !status_ok {
        let code = headers.lines().next().unwrap_or("").trim();
        return Err(AppError::Device(format!("{} rejected the UPnP {action}: {code}", addr.ip())));
    }
    Ok(body.to_string())
}

// ------------------------------------------------------------------ parsing --

/// Parse a `GetPositionInfo` response: `TrackDuration`, `RelTime`, and the
/// XML-escaped DIDL-Lite in `TrackMetaData`.
pub fn parse_position_info(xml: &str) -> NowPlaying {
    let (title, artist, album) = match tag(xml, "TrackMetaData") {
        Some(meta) => {
            let didl = unescape(&meta);
            (
                tag(&didl, "dc:title").unwrap_or_default(),
                tag(&didl, "upnp:artist").or_else(|| tag(&didl, "dc:creator")).unwrap_or_default(),
                tag(&didl, "upnp:album").unwrap_or_default(),
            )
        }
        None => (String::new(), String::new(), String::new()),
    };

    NowPlaying {
        title,
        artist,
        album,
        // A zero duration means "no track" here, not a zero-length track.
        duration_ms: tag(xml, "TrackDuration").and_then(parse_hms).filter(|&ms| ms > 0),
        position_ms: tag(xml, "RelTime").and_then(parse_hms),
    }
}

/// The text content of the first `<name>…</name>` element, trimmed. Tolerates
/// attributes on the opening tag (`<res duration="...">`).
fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let start = xml.find(&open)?;
    let after_open = xml[start..].find('>')? + start + 1;
    let close = format!("</{name}>");
    let end = xml[after_open..].find(&close)? + after_open;
    let inner = xml[after_open..end].trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

/// `H:MM:SS(.fff)` → milliseconds. UPnP uses this for both duration and position.
fn parse_hms(s: impl AsRef<str>) -> Option<u64> {
    let s = s.as_ref().trim();
    if s.is_empty() || s == "NOT_IMPLEMENTED" {
        return None;
    }
    let (hms, _frac) = s.split_once('.').unwrap_or((s, ""));
    let parts: Vec<&str> = hms.split(':').collect();
    let nums: Option<Vec<u64>> = parts.iter().map(|p| p.trim().parse::<u64>().ok()).collect();
    let nums = nums?;
    let secs = match nums.as_slice() {
        [h, m, s] => h * 3600 + m * 60 + s,
        [m, s] => m * 60 + s,
        [s] => *s,
        _ => return None,
    };
    Some(secs * 1000)
}

/// Minimal XML entity unescape for the escaped DIDL blob.
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_parse_accepts_all_verbs() {
        assert_eq!(Transport::parse("play"), Some(Transport::Play));
        assert_eq!(Transport::parse("PAUSE"), Some(Transport::Pause));
        assert_eq!(Transport::parse("next"), Some(Transport::Next));
        assert_eq!(Transport::parse("prev"), Some(Transport::Previous));
        assert_eq!(Transport::parse("previous"), Some(Transport::Previous));
        assert_eq!(Transport::parse("stop"), Some(Transport::Stop));
        assert_eq!(Transport::parse("seek"), None);
    }

    #[test]
    fn hms_parses_durations_and_positions() {
        assert_eq!(parse_hms("0:03:30"), Some(210_000));
        assert_eq!(parse_hms("1:00:00"), Some(3_600_000));
        assert_eq!(parse_hms("0:00:05.250"), Some(5_000));
        assert_eq!(parse_hms("2:07"), Some(127_000));
        assert_eq!(parse_hms("NOT_IMPLEMENTED"), None);
        assert_eq!(parse_hms(""), None);
    }

    #[test]
    fn position_info_pulls_didl_metadata() {
        let xml = r#"<?xml version="1.0"?><s:Envelope><s:Body>
            <u:GetPositionInfoResponse>
            <Track>1</Track>
            <TrackDuration>0:03:45</TrackDuration>
            <TrackMetaData>&lt;DIDL-Lite&gt;&lt;item&gt;&lt;dc:title&gt;Blue in Green&lt;/dc:title&gt;&lt;upnp:artist&gt;Miles Davis&lt;/upnp:artist&gt;&lt;upnp:album&gt;Kind of Blue&lt;/upnp:album&gt;&lt;/item&gt;&lt;/DIDL-Lite&gt;</TrackMetaData>
            <RelTime>0:01:12</RelTime>
            </u:GetPositionInfoResponse></s:Body></s:Envelope>"#;
        let np = parse_position_info(xml);
        assert_eq!(np.title, "Blue in Green");
        assert_eq!(np.artist, "Miles Davis");
        assert_eq!(np.album, "Kind of Blue");
        assert_eq!(np.duration_ms, Some(225_000));
        assert_eq!(np.position_ms, Some(72_000));
        assert!(!np.is_empty());
    }

    #[test]
    fn position_info_empty_when_stopped() {
        let xml = "<TrackDuration>0:00:00</TrackDuration><TrackMetaData></TrackMetaData><RelTime>0:00:00</RelTime>";
        let np = parse_position_info(xml);
        assert!(np.is_empty());
    }
}
