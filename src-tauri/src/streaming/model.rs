//! Types shared across the streaming core (Phase S2).
//!
//! The audio format is fixed to what `cliraop` expects on stdin: interleaved
//! **s16le, 44.1 kHz, stereo** (`RAOP_PCM`, 44100/16/2). Every frame is therefore
//! `BYTES_PER_FRAME` bytes, and the whole fan-out pipeline stays frame-aligned so
//! a delay/volume transform never splits a sample.

use serde::{Deserialize, Serialize};

/// RAOP PCM sample rate (Hz). `cliraop` is hard-wired to 44.1 kHz.
pub const SAMPLE_RATE: u32 = 44_100;
/// Interleaved channel count (stereo).
pub const CHANNELS: usize = 2;
/// Bits per sample per channel.
pub const BITS_PER_SAMPLE: usize = 16;
/// Bytes for one interleaved stereo frame: 2 channels × 2 bytes = 4.
pub const BYTES_PER_FRAME: usize = CHANNELS * (BITS_PER_SAMPLE / 8);

/// Default matched latency handed to every child (frames). The design doc uses
/// `MS2TS(500,44100)` = 22050 frames (500 ms) for the local rig; `cliraop`'s own
/// default is 1 s. Same value on every child ⇒ same DAC offset ⇒ speaker-to-
/// speaker lock.
pub const DEFAULT_LATENCY_FRAMES: u32 = 22_050;

/// Default warm-up (ms) before frame 0 hits the DAC. Same on every child.
pub const DEFAULT_WAIT_MS: u32 = 1_500;

/// Convert a millisecond duration to a whole number of PCM frames.
pub fn ms_to_frames(ms: u32) -> usize {
    (ms as u64 * SAMPLE_RATE as u64 / 1000) as usize
}

/// One RAOP receiver we fan a copy of the PCM stream out to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamTarget {
    /// Optional MusicSync device UUID, or a manual-target id (absent for the raw
    /// local test rig). Also the persistence key for this target's saved delay.
    #[serde(default)]
    pub uuid: Option<String>,
    /// Human label (also the shairport-sync `-a` name on the rig).
    pub name: String,
    /// Receiver IP (`127.0.0.1` for the local rig).
    pub ip: String,
    /// RAOP control port (`5000` for AirPlay 1; the rig uses distinct ports).
    pub raop_port: u16,
    /// Initial per-device playback delay in ms (Feature 2). Populated from the
    /// persisted store by `stream_start`; the frontend can omit it (defaults 0).
    #[serde(default)]
    pub delay_ms: u32,
}

impl StreamTarget {
    /// The key this target's persisted delay is stored under: its UUID/manual-id
    /// when present, else its IP. Mirrors [`crate::store::delay_key`].
    pub fn delay_key(&self) -> String {
        crate::store::delay_key(self.uuid.as_deref(), &self.ip)
    }
}

/// Where the PCM being streamed comes from (S2 = a local file/tone; the live
/// librespot ring buffer arrives in S3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamSource {
    /// A RIFF/WAVE file, decoded to s16le/44.1k/stereo.
    Wav { path: String },
    /// A headerless s16le/44.1k/stereo PCM file.
    RawPcm { path: String },
    /// A generated sine test tone (used by the sync-measurement example).
    Tone { freq_hz: u32, duration_ms: u32 },
    /// The live librespot capture (Phase S3): PCM is not loaded from a file but
    /// teed in real time from the Spotify manager's [`PcmFanout`](crate::streaming::live::PcmFanout).
    /// Carries no data — the engine pulls the fan-out from `AppState` at start.
    Spotify,
}

impl StreamSource {
    /// A short label for status/events.
    pub fn label(&self) -> String {
        match self {
            StreamSource::Wav { path } => format!("wav:{path}"),
            StreamSource::RawPcm { path } => format!("pcm:{path}"),
            StreamSource::Tone { freq_hz, duration_ms } => {
                format!("tone:{freq_hz}Hz/{duration_ms}ms")
            }
            StreamSource::Spotify => "spotify".to_string(),
        }
    }
}

/// Live per-receiver status, mirrored to the frontend inside [`StreamStatus`].
#[derive(Debug, Clone, Serialize)]
pub struct DeviceStatus {
    pub ip: String,
    pub name: String,
    pub raop_port: u16,
    /// Persistence key for this target's delay (device UUID / manual-id / IP), so
    /// the RoomRow can drive `set_target_delay` with the right key.
    pub key: String,
    /// Software volume gain in `0.0..=1.0`.
    pub volume: f32,
    /// Software delay applied ahead of this receiver's audio (ms).
    pub delay_ms: u32,
    /// Whether the child `cliraop` process is still alive.
    pub alive: bool,
    /// Frames pushed to this child's stdin so far (excludes delay silence).
    pub frames_written: u64,
}

/// Whole-group streaming status. Serialized into the `stream-state` event and
/// returned by the `stream_status` command.
#[derive(Debug, Clone, Serialize)]
pub struct StreamStatus {
    pub active: bool,
    pub source: Option<String>,
    /// The shared master NTP anchor captured once at start (decimal string).
    pub anchor_ntp: Option<String>,
    pub latency_frames: u32,
    pub devices: Vec<DeviceStatus>,
}

impl StreamStatus {
    /// The idle status (nothing streaming).
    pub fn idle() -> Self {
        Self {
            active: false,
            source: None,
            anchor_ntp: None,
            latency_frames: DEFAULT_LATENCY_FRAMES,
            devices: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_geometry_is_stereo_s16() {
        assert_eq!(BYTES_PER_FRAME, 4);
    }

    #[test]
    fn ms_to_frames_matches_sample_rate() {
        assert_eq!(ms_to_frames(1000), 44_100);
        assert_eq!(ms_to_frames(500), 22_050);
        assert_eq!(ms_to_frames(0), 0);
        // 10 ms at 44.1 kHz = 441 frames.
        assert_eq!(ms_to_frames(10), 441);
    }
}
