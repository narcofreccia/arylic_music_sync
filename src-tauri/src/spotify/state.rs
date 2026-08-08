//! Serializable Spotify state mirrored to the frontend (Phase S3).

use serde::Serialize;

use crate::spotify::meta::TrackMeta;

/// Coarse transport state of the connected Spotify session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayState {
    /// Nothing playing (or no client connected).
    #[default]
    Stopped,
    /// Audio is flowing.
    Playing,
    /// A client is connected but playback is paused.
    Paused,
}

/// The whole-endpoint Spotify state, serialized into the `spotify-state` event and
/// returned by `spotify_status`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SpotifyState {
    /// The "MusicSync" endpoint is advertising over zeroconf.
    pub running: bool,
    /// A Spotify client has taken over this endpoint.
    pub connected: bool,
    /// Current transport state.
    pub play_state: PlayState,
    /// Now-playing metadata, if a track has been loaded.
    pub track: Option<TrackMeta>,
    /// Last-reported playback position (ms).
    pub position_ms: u32,
    /// Connect volume on Spotify's 16-bit scale (0..=65535).
    pub volume: u16,
    /// The advertised device name (always "MusicSync" when running).
    pub device_name: String,
}
