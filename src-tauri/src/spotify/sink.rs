//! The custom librespot audio backend that captures PCM instead of playing it
//! (Phase S3).
//!
//! librespot decodes Spotify audio to interleaved **f64** samples and hands them
//! to an [`audio_backend::Sink`]. The stock backends (rodio/alsa/…) play to a
//! local sound card; ours does not — [`RingSink`] converts each packet to
//! interleaved **s16le** (via librespot's own [`Converter`], the exact geometry
//! `cliraop` wants: 44.1 kHz / 16-bit / stereo) and pushes it into the
//! [`PcmFanout`] tee that the streaming engine drains. There is no local
//! playback, no device, no latency of our own — just decode → convert → fan out.
//!
//! We request [`AudioFormat::S16`] so librespot's decode path and our RAOP sinks
//! agree on the sample format end to end; conversion is explicit little-endian so
//! the bytes are true s16le regardless of host endianness.

use std::sync::Arc;

use librespot_playback::audio_backend::{Sink, SinkError, SinkResult};
use librespot_playback::config::AudioFormat;
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;

use crate::streaming::live::PcmFanout;

/// A librespot `Sink` that tees decoded s16le PCM into the streaming engine.
pub struct RingSink {
    fanout: Arc<PcmFanout>,
    format: AudioFormat,
}

impl RingSink {
    /// Build a sink feeding `fanout`. The format is fixed to S16 — the one format
    /// the RAOP pipeline accepts — so any other request is a programming error.
    pub fn new(fanout: Arc<PcmFanout>) -> Self {
        Self {
            fanout,
            format: AudioFormat::S16,
        }
    }
}

impl Sink for RingSink {
    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        match packet {
            AudioPacket::Samples(samples) => {
                // f64 interleaved → s16 interleaved via librespot's dithering
                // converter, then serialised explicitly as little-endian so the
                // bytes are s16le on any host (matching cliraop's `RAOP_PCM`).
                debug_assert!(matches!(self.format, AudioFormat::S16));
                let s16 = converter.f64_to_s16(&samples);
                let bytes = s16_to_le_bytes(&s16);
                self.fanout.push(bytes);
                Ok(())
            }
            // Raw packets only occur with the passthrough decoder, which we never
            // enable; reject rather than forward an unknown container to RAOP.
            AudioPacket::Raw(_) => Err(SinkError::InvalidParams(
                "RingSink received a raw (passthrough) packet; expected decoded samples".into(),
            )),
        }
    }
}

/// Serialise interleaved `i16` samples as little-endian bytes (s16le).
pub fn s16_to_le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s16_serialises_little_endian_and_frame_sized() {
        // 0x0102 → [0x02, 0x01]; -1 → [0xFF, 0xFF].
        let bytes = s16_to_le_bytes(&[0x0102, -1, 0, i16::MIN]);
        assert_eq!(bytes, vec![0x02, 0x01, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x80]);
        // Two bytes per sample: a stereo frame is 4 bytes.
        assert_eq!(bytes.len() % 4, 0);
    }

    #[test]
    fn empty_input_yields_no_bytes() {
        assert!(s16_to_le_bytes(&[]).is_empty());
    }
}
