//! Live PCM fan-out bridging a real-time producer to the N per-device writers
//! (Phase S3).
//!
//! S2 streams a *static* PCM buffer: every `cliraop` writer independently
//! iterates the same `Arc<Vec<u8>>`. A live Spotify capture has no such buffer —
//! decoded frames arrive continuously from librespot's [`RingSink`](crate::spotify)
//! and must be *teed identically* to every receiver's writer (design doc §4, the
//! "tee the same PCM to every child" invariant that keeps the speakers locked).
//!
//! [`PcmFanout`] is that tee: a single-producer / multi-consumer broadcast. The
//! producer ([`push`](PcmFanout::push)) is librespot's decode thread; each
//! consumer is one device writer holding a bounded [`Receiver`]. Bounded channels
//! keep memory finite; on overflow we **drop** the packet for the lagging device
//! (staying real-time) rather than block the producer — a stalled receiver never
//! back-pressures the shared decode thread or its siblings, preserving the S2
//! per-device isolation. A disconnected receiver (its writer exited) is pruned on
//! the next push.
//!
//! The payload is `Arc<Vec<u8>>` so fanning one decoded packet out to N devices is
//! N cheap refcount bumps, not N copies; a writer only clones when it must scale
//! the bytes for a non-unity per-device volume.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

/// A decoded, interleaved-s16le PCM packet shared across all device writers.
pub type PcmPacket = Arc<Vec<u8>>;

/// Per-subscriber channel depth (packets). librespot emits ~one packet per
/// decoded frame batch (a few thousand frames); 128 packets is comfortably over
/// a second of buffer, so a briefly-busy writer never drops under normal pacing.
pub const DEFAULT_CAPACITY: usize = 128;

/// A single-producer, multi-consumer PCM broadcast (the live tee).
///
/// Created once and held by the Spotify manager for the process lifetime; the
/// streaming engine [`subscribe`](PcmFanout::subscribe)s one receiver per device
/// when a live stream starts and [`clear`](PcmFanout::clear)s them when it stops.
#[derive(Default)]
pub struct PcmFanout {
    subs: Mutex<Vec<SyncSender<PcmPacket>>>,
    pushed: AtomicU64,
    dropped: AtomicU64,
}

impl PcmFanout {
    /// A fresh fan-out with no subscribers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a consumer and return its bounded receiver. Called once per device
    /// by the engine at stream start.
    pub fn subscribe(&self, capacity: usize) -> Receiver<PcmPacket> {
        let (tx, rx) = sync_channel(capacity.max(1));
        if let Ok(mut subs) = self.subs.lock() {
            subs.push(tx);
        }
        rx
    }

    /// Drop every registered sender. Any writer still blocked on `recv` observes a
    /// disconnect and exits; a fresh session then re-subscribes from scratch.
    pub fn clear(&self) {
        if let Ok(mut subs) = self.subs.lock() {
            subs.clear();
        }
    }

    /// Number of live subscribers (after pruning happens on `push`).
    pub fn subscriber_count(&self) -> usize {
        self.subs.lock().map(|s| s.len()).unwrap_or(0)
    }

    /// Broadcast one PCM packet to every subscriber.
    ///
    /// Non-blocking: a full subscriber channel drops the packet *for that device
    /// only* (counted in [`dropped`](Self::dropped)); a disconnected subscriber is
    /// removed. The producer thread (librespot decode) is never blocked.
    pub fn push(&self, bytes: Vec<u8>) {
        let packet: PcmPacket = Arc::new(bytes);
        if let Ok(mut subs) = self.subs.lock() {
            subs.retain(|tx| match tx.try_send(packet.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            });
        }
        self.pushed.fetch_add(1, Ordering::Relaxed);
    }

    /// Total packets accepted from the producer.
    pub fn pushed(&self) -> u64 {
        self.pushed.load(Ordering::Relaxed)
    }

    /// Total per-device packet drops due to a full subscriber channel.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcasts_the_same_bytes_to_every_subscriber() {
        let f = PcmFanout::new();
        let a = f.subscribe(8);
        let b = f.subscribe(8);
        f.push(vec![1, 2, 3, 4]);
        assert_eq!(&**a.recv().unwrap(), &[1, 2, 3, 4]);
        assert_eq!(&**b.recv().unwrap(), &[1, 2, 3, 4]);
        assert_eq!(f.pushed(), 1);
        assert_eq!(f.dropped(), 0);
        assert_eq!(f.subscriber_count(), 2);
    }

    #[test]
    fn full_channel_drops_for_that_subscriber_without_blocking() {
        let f = PcmFanout::new();
        let rx = f.subscribe(2); // depth 2
        f.push(vec![0; 4]);
        f.push(vec![0; 4]);
        // Third push cannot fit → dropped, but push still returns.
        f.push(vec![0; 4]);
        assert_eq!(f.pushed(), 3);
        assert_eq!(f.dropped(), 1);
        // The two buffered packets are still retrievable.
        assert!(rx.recv().is_ok());
        assert!(rx.recv().is_ok());
    }

    #[test]
    fn disconnected_subscriber_is_pruned() {
        let f = PcmFanout::new();
        let rx = f.subscribe(4);
        drop(rx); // writer exited
        f.push(vec![9, 9, 9, 9]);
        assert_eq!(f.subscriber_count(), 0);
    }

    #[test]
    fn clear_removes_all_subscribers() {
        let f = PcmFanout::new();
        let _a = f.subscribe(4);
        let _b = f.subscribe(4);
        assert_eq!(f.subscriber_count(), 2);
        f.clear();
        assert_eq!(f.subscriber_count(), 0);
    }

    #[test]
    fn arc_payload_is_shared_not_copied() {
        let f = PcmFanout::new();
        let a = f.subscribe(4);
        let b = f.subscribe(4);
        f.push(vec![7; 16]);
        let pa = a.recv().unwrap();
        let pb = b.recv().unwrap();
        assert!(Arc::ptr_eq(&pa, &pb));
    }
}
