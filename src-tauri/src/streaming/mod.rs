//! Synchronized RAOP multi-sender core (Phase S2).
//!
//! Pushes one PCM source to N AirPlay-1/RAOP receivers in tight sync by driving
//! one bundled `cliraop` child per receiver off a single shared NTP master clock
//! (see `docs/STREAMING-design.md`). Spotify/librespot (S3) and UI (S5) are out of
//! scope here; the source is a local WAV/PCM/tone, validated against local
//! `shairport-sync` receivers.
//!
//! Layout:
//! * [`model`]  — audio geometry, targets, status types.
//! * [`sync`]   — pure PCM-domain transforms (volume, delay, framing).
//! * [`wav`]    — WAV/RAW/tone source loading (no external audio crate).
//! * [`sidecar`]— one `cliraop` child per receiver + shared-NTP capture.
//! * [`engine`] — the orchestrator: anchor once, spawn N, tee PCM with per-device DSP.

pub mod engine;
pub mod live;
pub mod model;
pub mod sidecar;
pub mod sync;
pub mod wav;

pub use engine::StreamEngine;
