//! Linkplay HTTP API layer: typed commands, tolerant response models and the
//! shared client. Nothing above this module writes an API string by hand.

pub mod client;
pub mod hexstr;
pub mod models;

pub use client::{LinkplayClient, LinkplayCommand};
pub use models::{DeviceDetail, DeviceRole, DeviceSnapshot, PlayerStatus, SlaveList, StatusEx};
