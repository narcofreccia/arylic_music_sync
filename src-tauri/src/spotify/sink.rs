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
use std::time::{Duration, Instant};

use librespot_playback::audio_backend::{Sink, SinkError, SinkResult};
use librespot_playback::config::AudioFormat;
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;

use crate::streaming::live::PcmFanout;

/// librespot decodes to this fixed geometry (44.1 kHz interleaved stereo).
const SAMPLE_RATE: u64 = 44_100;
const CHANNELS: u64 = 2;
/// How far ahead of wall-clock the decoder may run before we throttle it. A real
/// sound card paces playback; with no local device librespot would otherwise
/// decode a whole track in a second or two and auto-advance. We let it run this
/// far ahead so the RAOP sinks have audio to prime their ~1.5 s buffers, then
/// pace it to 1× so tracks play at normal speed whether or not speakers are
/// attached.
const LEAD: Duration = Duration::from_millis(2000);

/// A librespot `Sink` that tees decoded s16le PCM into the streaming engine.
pub struct RingSink {
    fanout: Arc<PcmFanout>,
    format: AudioFormat,
    /// Wall-clock origin of the current playback run (reset on start/stop).
    clock_start: Option<Instant>,
    /// Frames handed downstream since `clock_start` — the playout position.
    frames_written: u64,
}

impl RingSink {
    /// Build a sink feeding `fanout`. The format is fixed to S16 — the one format
    /// the RAOP pipeline accepts — so any other request is a programming error.
    pub fn new(fanout: Arc<PcmFanout>) -> Self {
        Self {
            fanout,
            format: AudioFormat::S16,
            clock_start: None,
            frames_written: 0,
        }
    }

    /// Throttle the decode thread to real time after `frame_count` more frames,
    /// allowing up to `LEAD` of buffer-priming lead. This is what stops Spotify
    /// from racing through tracks when nothing is consuming the audio.
    fn pace(&mut self, frame_count: u64) {
        let start = *self.clock_start.get_or_insert_with(Instant::now);
        self.frames_written += frame_count;
        let playout =
            Duration::from_nanos(self.frames_written * 1_000_000_000 / SAMPLE_RATE);
        let target = start + playout;
        let now = Instant::now();
        // Sleep only if we've run more than LEAD ahead of the wall clock.
        if let Some(ahead) = target.checked_duration_since(now) {
            if ahead > LEAD {
                std::thread::sleep(ahead - LEAD);
            }
        }
    }
}

impl Sink for RingSink {
    fn start(&mut self) -> SinkResult<()> {
        // New playback run — reset the real-time clock so pacing tracks it.
        self.clock_start = None;
        self.frames_written = 0;
        Ok(())
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.clock_start = None;
        self.frames_written = 0;
        Ok(())
    }

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
                // Pace to 1× real time so tracks don't auto-advance.
                self.pace(s16.len() as u64 / CHANNELS);
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
