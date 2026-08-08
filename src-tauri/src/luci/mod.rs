//! LibreWireless **Luci** control protocol (TCP `<ip>:7777` + TLS 1.2, mutual
//! auth). The native local-control channel for the LP10 — the LP10 does not
//! speak the classic Linkplay httpapi (docs/firmware-notes.md).
//!
//! Layering, bottom to top: [`cert`] embeds the client identity, [`tls`] builds
//! the rustls connector, [`codec`] frames bytes, [`messagebox`] names commands,
//! [`client`] holds one persistent connection per device and correlates
//! replies, and [`model`] parses payloads into the app's `DeviceSnapshot`.

pub mod cert;
pub mod client;
pub mod codec;
pub mod messagebox;
pub mod model;
pub mod tls;

pub use client::{LuciClient, LuciEvent};
pub use messagebox::{MessageBox, MessageType};
pub use model::{DdmsBanner, DevInfo, DeviceDetail, DeviceSnapshot, NetMode, Role, Track};
