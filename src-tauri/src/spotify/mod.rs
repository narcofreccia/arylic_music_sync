//! Spotify capture via librespot (Phase S3).
//!
//! Turns MusicSync into a Spotify **Connect** endpoint named "MusicSync": the user
//! selects it in their Spotify app and authorizes it (zeroconf discovery mode — no
//! stored password), and we capture the decoded audio as interleaved s16le/44.1k/
//! stereo PCM plus now-playing metadata, feeding the PCM into the S2 streaming
//! engine's live fan-out and surfacing state to the UI via `spotify-state` events.
//!
//! Layout:
//! * [`manager`] — the session lifecycle: runtime, zeroconf advertise, event pump,
//!   transport control (`SpotifyManager`, held in `AppState`).
//! * [`sink`]    — the custom librespot audio backend that captures PCM instead of
//!   playing it (`RingSink`).
//! * [`meta`]    — now-playing metadata mapping (pure, unit-tested).
//! * [`state`]   — the serializable `SpotifyState` mirrored to the frontend.
//!
//! ## Legal / ToS note
//! librespot is an **unofficial, reverse-engineered** Spotify client. Spotify's
//! Terms of Service do not sanction third-party clients, so this is a ToS gray area
//! (a conscious product decision, not a technical blocker), and it requires a
//! Spotify **Premium** account to function at all. See `docs/AUDIO-STREAMING-
//! feasibility.md` and `docs/STREAMING-design.md` §3.

pub mod manager;
pub mod meta;
pub mod sink;
pub mod state;

pub use manager::{SpotifyManager, DEVICE_NAME};
pub use meta::TrackMeta;
pub use state::{PlayState, SpotifyState};
