//! Pure PCM-domain transforms for the per-device fan-out (Phase S2).
//!
//! Everything here is deliberately side-effect-free and frame-agnostic beyond
//! the s16le stereo geometry, so it can be unit-tested without spawning a child
//! or touching the network. The engine (`engine.rs`) applies these to each
//! child's byte stream just before writing to its stdin:
//!
//! * [`scale_s16le`] — per-device **software volume** (§4 of the design doc).
//! * [`silence_frames`] — the head of a per-device **delay line**.
//! * [`FrameChunks`] — frame-aligned chunking so a transform never splits a
//!   16-bit sample across two writes.
//!
//! The actual cross-room *timing* lock is not done here — that lives in the
//! shared NTP anchor + matched latency handed to every `cliraop` child. These
//! transforms only shape amplitude and add a deliberate per-room offset.

use super::model::BYTES_PER_FRAME;

/// Scale interleaved s16le samples in place by `gain` (`0.0..=1.0`).
///
/// `gain` is clamped to `0.0..=1.0` (we never amplify — a >1.0 gain would clip
/// hard on a 16-bit sink). `1.0` is a no-op fast path. Rounding is
/// round-half-away-from-zero via `.round()`, and the result is saturated into
/// `i16` range for safety even though attenuation alone cannot overflow.
///
/// `bytes.len()` need not be frame-aligned — it is processed as a flat sequence
/// of little-endian `i16` samples; a trailing odd byte (which cannot occur on a
/// frame-aligned buffer) is left untouched.
pub fn scale_s16le(bytes: &mut [u8], gain: f32) {
    let gain = gain.clamp(0.0, 1.0);
    if gain == 1.0 {
        return;
    }
    for sample in bytes.chunks_exact_mut(2) {
        let v = i16::from_le_bytes([sample[0], sample[1]]);
        let scaled = (v as f32 * gain).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        let out = scaled.to_le_bytes();
        sample[0] = out[0];
        sample[1] = out[1];
    }
}

/// A buffer of `frames` silent stereo frames (all-zero s16le), used as the head
/// of a per-device delay line: prepending it to a child's stream shifts that
/// receiver's audio later by exactly `frames / 44100` seconds without disturbing
/// any other child.
pub fn silence_frames(frames: usize) -> Vec<u8> {
    vec![0u8; frames * BYTES_PER_FRAME]
}

/// Round a byte length down to a whole number of frames.
pub fn frame_align(len: usize) -> usize {
    len - (len % BYTES_PER_FRAME)
}

/// Iterator that yields frame-aligned slices of at most `chunk_frames` frames
/// from a PCM byte buffer. The final slice may be shorter but is always a whole
/// number of frames (assuming the input is frame-aligned).
pub struct FrameChunks<'a> {
    data: &'a [u8],
    step: usize,
    pos: usize,
}

impl<'a> FrameChunks<'a> {
    pub fn new(data: &'a [u8], chunk_frames: usize) -> Self {
        let step = chunk_frames.max(1) * BYTES_PER_FRAME;
        Self { data, step, pos: 0 }
    }
}

impl<'a> Iterator for FrameChunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.pos >= self.data.len() {
            return None;
        }
        let end = (self.pos + self.step).min(self.data.len());
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Some(slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s16(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }
    fn de16(bytes: &[u8]) -> Vec<i16> {
        bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()
    }

    #[test]
    fn unity_gain_is_identity() {
        let mut b = s16(&[1000, -2000, 32767, -32768]);
        let orig = b.clone();
        scale_s16le(&mut b, 1.0);
        assert_eq!(b, orig);
    }

    #[test]
    fn half_gain_halves_amplitude() {
        let mut b = s16(&[1000, -2000, 32766, -32768]);
        scale_s16le(&mut b, 0.5);
        assert_eq!(de16(&b), vec![500, -1000, 16383, -16384]);
    }

    #[test]
    fn zero_gain_mutes() {
        let mut b = s16(&[12345, -12345, 32767, -32768]);
        scale_s16le(&mut b, 0.0);
        assert_eq!(de16(&b), vec![0, 0, 0, 0]);
    }

    #[test]
    fn gain_is_clamped_and_never_amplifies() {
        // >1.0 must clamp to unity, not clip the full-scale sample.
        let mut b = s16(&[20000, -20000]);
        scale_s16le(&mut b, 4.0);
        assert_eq!(de16(&b), vec![20000, -20000]);
        // Negative gain clamps to silence.
        let mut b2 = s16(&[20000, -20000]);
        scale_s16le(&mut b2, -1.0);
        assert_eq!(de16(&b2), vec![0, 0]);
    }

    #[test]
    fn silence_is_frame_sized_and_zero() {
        let s = silence_frames(441); // 10 ms
        assert_eq!(s.len(), 441 * BYTES_PER_FRAME);
        assert!(s.iter().all(|&b| b == 0));
        assert_eq!(silence_frames(0).len(), 0);
    }

    #[test]
    fn frame_align_drops_partial_frame() {
        assert_eq!(frame_align(0), 0);
        assert_eq!(frame_align(4), 4);
        assert_eq!(frame_align(5), 4);
        assert_eq!(frame_align(7), 4);
        assert_eq!(frame_align(8), 8);
    }

    #[test]
    fn frame_chunks_are_frame_aligned_and_lossless() {
        // 10 frames of data, chunk of 3 frames → 3+3+3+1.
        let data = vec![7u8; 10 * BYTES_PER_FRAME];
        let chunks: Vec<&[u8]> = FrameChunks::new(&data, 3).collect();
        assert_eq!(chunks.len(), 4);
        for c in &chunks[..3] {
            assert_eq!(c.len(), 3 * BYTES_PER_FRAME);
        }
        assert_eq!(chunks[3].len(), BYTES_PER_FRAME);
        // Reassembling yields the original bytes exactly.
        let joined: Vec<u8> = chunks.concat();
        assert_eq!(joined, data);
    }

    #[test]
    fn frame_chunks_handles_empty_and_exact() {
        assert_eq!(FrameChunks::new(&[], 4).count(), 0);
        let data = vec![1u8; 8 * BYTES_PER_FRAME];
        assert_eq!(FrameChunks::new(&data, 4).count(), 2);
    }
}
