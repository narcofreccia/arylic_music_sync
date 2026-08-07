//! The typed Linkplay HTTP client (NFR-4/NFR-6).
//!
//! Every device call in the app goes through here, and every piece of command
//! *syntax* lives in [`LinkplayCommand::to_query`] — one place to fix when the
//! FR-23 spike tells us a firmware wants something slightly different.

use std::time::Duration;

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::de::DeserializeOwned;

use crate::error::{AppError, AppResult};

/// Per-request timeout (brief NFR-3: ~2 s, so one dead device never stalls a
/// polling cycle).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// Retry schedule for the commands that mutate group state (FR-14). Defined
/// here so M4 inherits the tuned values rather than inventing its own.
const RETRY_BACKOFF_MS: [u64; 3] = [200, 600, 1400];

/// Percent-encoding set for `setDeviceName`: keep the unreserved characters,
/// encode everything else (spaces, accents, emoji — see firmware-notes §8).
const NAME_ENCODE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

/// Which join syntax to send. Firmware generations disagree (brief §9), so the
/// choice is explicit rather than hidden in a string.
///
/// M2 only ever uses [`JoinVariant::Eth`]. The M2 hardware spike
/// (docs/firmware-notes.md §4) decides the canonical variant for M4's grouping;
/// until it is filled in, do not change the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinVariant {
    /// Canonical form from brief §3, sent to the *slave*.
    Eth,
    /// Same payload under the `multiroom:` namespace, seen on newer builds.
    MultiroomEth,
    /// Legacy SoftAP form. Carries the master's hex-encoded SSID because only
    /// the caller can know it — it is not derivable from the IP.
    LegacySsid { ssid_hex: String },
}

/// Transport controls (FR-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportCmd {
    Pause,
    Resume,
    /// Play/pause toggle — the one most remotes map to a single button.
    OnePause,
    Stop,
    Prev,
    Next,
}

impl TransportCmd {
    fn as_str(self) -> &'static str {
        match self {
            TransportCmd::Pause => "pause",
            TransportCmd::Resume => "resume",
            TransportCmd::OnePause => "onepause",
            TransportCmd::Stop => "stop",
            TransportCmd::Prev => "prev",
            TransportCmd::Next => "next",
        }
    }
}

/// The complete set of device commands the app knows how to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkplayCommand {
    GetStatusEx,
    GetPlayerStatus,
    GetSlaveList,
    /// Sent to the *slave* that should join `master_ip`.
    JoinGroup { master_ip: String, variant: JoinVariant },
    /// Leave (on a slave) or dissolve (on a master) — exact semantics are a
    /// spike question (firmware-notes §5).
    Ungroup,
    /// Sent to the master to eject one member.
    KickSlave { slave_ip: String },
    SlaveVolume { slave_ip: String, vol: u8 },
    SlaveMute { slave_ip: String, mute: bool },
    SetVolume(u8),
    SetMute(bool),
    Transport(TransportCmd),
    /// Absolute position, in seconds (the API takes seconds, not ms).
    Seek(u32),
    SetDeviceName(String),
}

impl LinkplayCommand {
    /// The `command=` value. **The single place Linkplay syntax is written.**
    pub fn to_query(&self) -> String {
        match self {
            LinkplayCommand::GetStatusEx => "getStatusEx".to_string(),
            LinkplayCommand::GetPlayerStatus => "getPlayerStatus".to_string(),
            LinkplayCommand::GetSlaveList => "multiroom:getSlaveList".to_string(),
            LinkplayCommand::JoinGroup { master_ip, variant } => match variant {
                JoinVariant::Eth => {
                    format!("ConnectMasterAp:JoinGroupMaster:eth{master_ip}:wifi0.0.0.0")
                }
                JoinVariant::MultiroomEth => {
                    format!("multiroom:JoinGroupMaster:eth{master_ip}:wifi0.0.0.0")
                }
                JoinVariant::LegacySsid { ssid_hex } => {
                    format!("ConnectMasterAp:ssid={ssid_hex}:ch=0:auth=OPEN:encry=NONE:pwd=:chext=0")
                }
            },
            LinkplayCommand::Ungroup => "multiroom:Ungroup".to_string(),
            LinkplayCommand::KickSlave { slave_ip } => format!("multiroom:SlaveKickout:{slave_ip}"),
            LinkplayCommand::SlaveVolume { slave_ip, vol } => {
                format!("multiroom:SlaveVolume:{slave_ip}:{}", (*vol).min(100))
            }
            LinkplayCommand::SlaveMute { slave_ip, mute } => {
                format!("multiroom:SlaveMute:{slave_ip}:{}", u8::from(*mute))
            }
            LinkplayCommand::SetVolume(vol) => format!("setPlayerCmd:vol:{}", (*vol).min(100)),
            LinkplayCommand::SetMute(mute) => format!("setPlayerCmd:mute:{}", u8::from(*mute)),
            LinkplayCommand::Transport(cmd) => format!("setPlayerCmd:{}", cmd.as_str()),
            LinkplayCommand::Seek(secs) => format!("setPlayerCmd:seek:{secs}"),
            LinkplayCommand::SetDeviceName(name) => {
                format!("setDeviceName:{}", utf8_percent_encode(name, NAME_ENCODE))
            }
        }
    }
}

/// Shared HTTP client for every device call. Cloning is cheap (reqwest pools
/// internally), so `AppState` hands out clones freely.
#[derive(Debug, Clone)]
pub struct LinkplayClient {
    http: reqwest::Client,
}

impl LinkplayClient {
    pub fn new(timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            // CRITICAL: a system/corporate HTTP proxy would happily swallow
            // these LAN calls (or answer them with an error page), and the user
            // would see every speaker as offline for no visible reason.
            // MusicSync only ever talks to private addresses — never proxy.
            .no_proxy()
            // Devices are polled every few seconds; keeping the connection warm
            // saves a handshake per poll per device.
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|e| {
                // Only fails if the TLS/DNS backend can't initialise; a default
                // client still beats bringing the app down at startup.
                log::error!("falling back to a default HTTP client: {e}");
                reqwest::Client::new()
            });
        Self { http }
    }

    /// `GET http://<ip>/httpapi.asp?command=<query>`, returning the raw body.
    pub async fn send(&self, ip: &str, cmd: &LinkplayCommand) -> AppResult<String> {
        let query = cmd.to_query();
        let url = format!("http://{ip}/httpapi.asp?command={query}");
        let response = self.http.get(&url).send().await.map_err(|e| {
            AppError::Device(if e.is_timeout() {
                format!("{ip} did not answer within {}s.", DEFAULT_TIMEOUT.as_secs())
            } else {
                format!("{ip} is unreachable: {e}")
            })
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(AppError::Device(format!("{ip} answered {status} to `{query}`.")));
        }
        response
            .text()
            .await
            .map_err(|e| AppError::Device(format!("{ip} sent an unreadable body: {e}")))
    }

    /// Send and parse. The body is parsed from text rather than via
    /// `Response::json` because Linkplay answers JSON with `text/html`.
    pub async fn send_json<T: DeserializeOwned>(&self, ip: &str, cmd: &LinkplayCommand) -> AppResult<T> {
        let body = self.send(ip, cmd).await?;
        serde_json::from_str(body.trim()).map_err(|e| {
            AppError::Device(format!("{ip} sent an unparseable response to `{}`: {e}", cmd.to_query()))
        })
    }

    /// Send a command whose only valid answer is `OK`.
    #[allow(dead_code)] // used by M4 (grouping) / M5 (volume, transport)
    pub async fn send_ok(&self, ip: &str, cmd: &LinkplayCommand) -> AppResult<()> {
        let body = self.send(ip, cmd).await?;
        if body.trim().eq_ignore_ascii_case("OK") {
            Ok(())
        } else {
            Err(AppError::Device(format!(
                "{ip} rejected `{}`: {}",
                cmd.to_query(),
                body.trim()
            )))
        }
    }

    /// Retry with backoff (FR-14). Deliberately *not* used by the poller: a
    /// polling cycle that retries would smear failures across cycles and blur
    /// the offline threshold. This is for the state-changing commands M4/M5 add.
    #[allow(dead_code)] // used by M4 (grouping)
    pub async fn send_with_retry(&self, ip: &str, cmd: &LinkplayCommand) -> AppResult<String> {
        let mut last = None;
        for (attempt, delay) in RETRY_BACKOFF_MS.iter().enumerate() {
            match self.send(ip, cmd).await {
                Ok(body) => return Ok(body),
                Err(e) => {
                    log::warn!("{ip}: `{}` attempt {} failed: {e}", cmd.to_query(), attempt + 1);
                    last = Some(e);
                    // No sleep after the final attempt.
                    if attempt + 1 < RETRY_BACKOFF_MS.len() {
                        tokio::time::sleep(Duration::from_millis(*delay)).await;
                    }
                }
            }
        }
        Err(last.unwrap_or_else(|| AppError::Device(format!("{ip} did not answer."))))
    }
}

impl Default for LinkplayClient {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_commands_match_the_documented_syntax() {
        assert_eq!(LinkplayCommand::GetStatusEx.to_query(), "getStatusEx");
        assert_eq!(LinkplayCommand::GetPlayerStatus.to_query(), "getPlayerStatus");
        assert_eq!(LinkplayCommand::GetSlaveList.to_query(), "multiroom:getSlaveList");
    }

    #[test]
    fn join_variants_render_their_own_syntax() {
        let eth = LinkplayCommand::JoinGroup {
            master_ip: "192.168.10.20".into(),
            variant: JoinVariant::Eth,
        };
        assert_eq!(
            eth.to_query(),
            "ConnectMasterAp:JoinGroupMaster:eth192.168.10.20:wifi0.0.0.0"
        );

        let multi = LinkplayCommand::JoinGroup {
            master_ip: "192.168.10.20".into(),
            variant: JoinVariant::MultiroomEth,
        };
        assert_eq!(multi.to_query(), "multiroom:JoinGroupMaster:eth192.168.10.20:wifi0.0.0.0");

        let legacy = LinkplayCommand::JoinGroup {
            master_ip: "192.168.10.20".into(),
            variant: JoinVariant::LegacySsid { ssid_hex: "4c503130".into() },
        };
        assert_eq!(
            legacy.to_query(),
            "ConnectMasterAp:ssid=4c503130:ch=0:auth=OPEN:encry=NONE:pwd=:chext=0"
        );
    }

    #[test]
    fn group_commands_match_the_documented_syntax() {
        assert_eq!(LinkplayCommand::Ungroup.to_query(), "multiroom:Ungroup");
        assert_eq!(
            LinkplayCommand::KickSlave { slave_ip: "192.168.10.21".into() }.to_query(),
            "multiroom:SlaveKickout:192.168.10.21"
        );
        assert_eq!(
            LinkplayCommand::SlaveVolume { slave_ip: "192.168.10.21".into(), vol: 42 }.to_query(),
            "multiroom:SlaveVolume:192.168.10.21:42"
        );
        assert_eq!(
            LinkplayCommand::SlaveMute { slave_ip: "192.168.10.21".into(), mute: true }.to_query(),
            "multiroom:SlaveMute:192.168.10.21:1"
        );
        assert_eq!(
            LinkplayCommand::SlaveMute { slave_ip: "192.168.10.21".into(), mute: false }.to_query(),
            "multiroom:SlaveMute:192.168.10.21:0"
        );
    }

    #[test]
    fn player_commands_match_the_documented_syntax() {
        assert_eq!(LinkplayCommand::SetVolume(75).to_query(), "setPlayerCmd:vol:75");
        assert_eq!(LinkplayCommand::SetMute(true).to_query(), "setPlayerCmd:mute:1");
        assert_eq!(LinkplayCommand::SetMute(false).to_query(), "setPlayerCmd:mute:0");
        assert_eq!(LinkplayCommand::Seek(125).to_query(), "setPlayerCmd:seek:125");
        for (cmd, expected) in [
            (TransportCmd::Pause, "setPlayerCmd:pause"),
            (TransportCmd::Resume, "setPlayerCmd:resume"),
            (TransportCmd::OnePause, "setPlayerCmd:onepause"),
            (TransportCmd::Stop, "setPlayerCmd:stop"),
            (TransportCmd::Prev, "setPlayerCmd:prev"),
            (TransportCmd::Next, "setPlayerCmd:next"),
        ] {
            assert_eq!(LinkplayCommand::Transport(cmd).to_query(), expected);
        }
    }

    #[test]
    fn volume_is_clamped_to_the_api_range() {
        assert_eq!(LinkplayCommand::SetVolume(255).to_query(), "setPlayerCmd:vol:100");
        assert_eq!(
            LinkplayCommand::SlaveVolume { slave_ip: "1.2.3.4".into(), vol: 200 }.to_query(),
            "multiroom:SlaveVolume:1.2.3.4:100"
        );
    }

    #[test]
    fn device_name_is_url_encoded() {
        // FR-22 pushes names with spaces, accents and emoji.
        assert_eq!(
            LinkplayCommand::SetDeviceName("Whole House".into()).to_query(),
            "setDeviceName:Whole%20House"
        );
        assert_eq!(
            LinkplayCommand::SetDeviceName("Cucina".into()).to_query(),
            "setDeviceName:Cucina"
        );
        // A colon in the name must not look like another command segment.
        assert_eq!(
            LinkplayCommand::SetDeviceName("A:B".into()).to_query(),
            "setDeviceName:A%3AB"
        );
        assert_eq!(
            LinkplayCommand::SetDeviceName("Café".into()).to_query(),
            "setDeviceName:Caf%C3%A9"
        );
    }
}
