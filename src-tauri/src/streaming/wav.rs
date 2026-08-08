//! Minimal PCM source loading for S2: a headerless-RAW passthrough, a tiny
//! RIFF/WAVE reader, and a sine-tone generator — all producing the one format
//! `cliraop` wants (`s16le`, 44.1 kHz, stereo). No external audio crate: the
//! WAV subset we accept is exactly what `ffmpeg -f wav` / the generator below
//! emit, and anything else is rejected rather than mis-decoded.

use std::f64::consts::PI;
use std::fs;
use std::path::Path;

use super::model::{StreamSource, BYTES_PER_FRAME, CHANNELS, SAMPLE_RATE};

/// Load a [`StreamSource`] into raw interleaved s16le/44.1k/stereo bytes.
pub fn load_source(source: &StreamSource) -> Result<Vec<u8>, String> {
    match source {
        StreamSource::RawPcm { path } => {
            let bytes = fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
            Ok(frame_align_owned(bytes))
        }
        StreamSource::Wav { path } => {
            let bytes = fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
            parse_wav_s16le(&bytes)
        }
        StreamSource::Tone { freq_hz, duration_ms } => {
            Ok(sine_tone(*freq_hz, *duration_ms))
        }
    }
}

/// Truncate a byte buffer to a whole number of frames.
fn frame_align_owned(mut bytes: Vec<u8>) -> Vec<u8> {
    let keep = bytes.len() - (bytes.len() % BYTES_PER_FRAME);
    bytes.truncate(keep);
    bytes
}

/// Parse the PCM `data` out of a canonical little-endian RIFF/WAVE file, requiring
/// 16-bit / 44.1 kHz / 2-channel `WAVE_FORMAT_PCM`. Chunks are walked so a `LIST`/
/// `fact` chunk ahead of `data` (ffmpeg emits neither by default, but be lenient)
/// does not break parsing.
pub fn parse_wav_s16le(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut pos = 12;
    let mut fmt_ok = false;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        let body = pos + 8;
        if id == b"fmt " {
            if body + 16 > bytes.len() {
                return Err("truncated fmt chunk".into());
            }
            let audio_format = u16::from_le_bytes([bytes[body], bytes[body + 1]]);
            let channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            let rate = u32::from_le_bytes([bytes[body + 4], bytes[body + 5], bytes[body + 6], bytes[body + 7]]);
            let bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
            if audio_format != 1 {
                return Err(format!("unsupported WAVE format {audio_format} (need PCM=1)"));
            }
            if channels as usize != CHANNELS || rate != SAMPLE_RATE || bits != 16 {
                return Err(format!(
                    "unsupported WAV geometry: {channels}ch/{rate}Hz/{bits}bit (need {CHANNELS}ch/{SAMPLE_RATE}Hz/16bit)"
                ));
            }
            fmt_ok = true;
        } else if id == b"data" {
            if !fmt_ok {
                return Err("data chunk before fmt chunk".into());
            }
            let end = (body + size).min(bytes.len());
            return Ok(frame_align_owned(bytes[body..end].to_vec()));
        }
        // Chunks are word-aligned: an odd size carries a pad byte.
        pos = body + size + (size & 1);
    }
    Err("no data chunk found".into())
}

/// Generate an interleaved s16le stereo sine tone. Amplitude is ~0.5 full-scale
/// so a downstream volume scale still has headroom. Both channels carry the same
/// signal, which keeps the cross-correlation sync measurement clean.
pub fn sine_tone(freq_hz: u32, duration_ms: u32) -> Vec<u8> {
    let total_frames = (SAMPLE_RATE as u64 * duration_ms as u64 / 1000) as usize;
    let amp = 16000.0_f64;
    let mut out = Vec::with_capacity(total_frames * BYTES_PER_FRAME);
    let step = 2.0 * PI * freq_hz as f64 / SAMPLE_RATE as f64;
    for n in 0..total_frames {
        let v = (amp * (step * n as f64).sin()).round() as i16;
        let le = v.to_le_bytes();
        out.extend_from_slice(&le); // left
        out.extend_from_slice(&le); // right
    }
    out
}

/// Write a canonical 44-byte-header WAV around raw s16le/44.1k/stereo PCM.
/// Used by the example to hand a real `.wav` file to the engine.
pub fn write_wav(path: &Path, pcm: &[u8]) -> Result<(), String> {
    let data_len = pcm.len() as u32;
    let byte_rate = SAMPLE_RATE * (CHANNELS as u32) * 2;
    let block_align = (CHANNELS as u16) * 2;
    let mut buf = Vec::with_capacity(44 + pcm.len());
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&(CHANNELS as u16).to_le_bytes());
    buf.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    buf.extend_from_slice(pcm);
    fs::write(path, &buf).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_has_expected_length_and_is_frame_aligned() {
        let pcm = sine_tone(1000, 100); // 100 ms
        assert_eq!(pcm.len(), 4410 * BYTES_PER_FRAME);
        assert_eq!(pcm.len() % BYTES_PER_FRAME, 0);
    }

    #[test]
    fn wav_roundtrips_through_writer_and_parser() {
        let pcm = sine_tone(440, 50);
        let dir = std::env::temp_dir();
        let path = dir.join("music_sync_wav_roundtrip_test.wav");
        write_wav(&path, &pcm).unwrap();
        let parsed = parse_wav_s16le(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(parsed, pcm);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wav_rejects_wrong_geometry() {
        // Hand-build a mono 22050 Hz header — must be rejected, not mis-read.
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&36u32.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // mono
        buf.extend_from_slice(&22050u32.to_le_bytes());
        buf.extend_from_slice(&44100u32.to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&0u32.to_le_bytes());
        assert!(parse_wav_s16le(&buf).is_err());
    }

    #[test]
    fn wav_rejects_non_riff() {
        assert!(parse_wav_s16le(b"not a wav file at all").is_err());
    }
}
