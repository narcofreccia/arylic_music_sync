//! `LuciClient` — a persistent, TLS-authenticated connection to one device's
//! Luci endpoint (`<ip>:7777`).
//!
//! One connection per device, held open for its whole lifetime (matching the
//! SDK). A single reader task demuxes the stream: frames whose command matches a
//! pending [`request`](LuciClient::request) are delivered to that caller; every
//! other frame is a device-initiated **push** (e.g. `GETPLAYDURATION`,
//! `DEVICESTATE`) and is forwarded to the event channel returned by
//! [`connect`](LuciClient::connect).
//!
//! Requests are serialized on the client, so correlating a reply by its command
//! id is unambiguous — the poller reads one value at a time. Reconnection is the
//! poller's job: when the connection drops, [`request`] fails and the poller
//! rebuilds the client with backoff.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex as TokioMutex};
use tokio_rustls::client::TlsStream;

use crate::error::{AppError, AppResult};
use crate::luci::codec::{self, Frame};
use crate::luci::messagebox::{MessageBox, MessageType};
use crate::luci::tls;

/// The Luci control port.
pub const LUCI_PORT: u16 = 7777;

/// How long a single request waits for its correlated reply.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the TCP+TLS handshake may take before we give up on a device.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);

/// Bound on the async-event channel — pushes are advisory (the poller reads
/// authoritative state on its own cadence), so dropping the oldest under a burst
/// is fine.
const EVENT_CHANNEL_CAPACITY: usize = 64;

/// A device-initiated push: the decoded command, its status, and the payload.
pub type LuciEvent = (MessageBox, u8, String);

/// Shared connection state. The reader task and the request path both hold an
/// `Arc<Inner>`.
struct Inner {
    ip: String,
    write: TokioMutex<WriteHalf<TlsStream<TcpStream>>>,
    /// Callers waiting on a correlated reply, keyed by command id.
    pending: StdMutex<HashMap<u16, oneshot::Sender<Frame>>>,
    /// Set once the reader task has seen EOF or an error; every later request
    /// fails fast instead of waiting out the timeout.
    closed: AtomicBool,
    /// Serializes requests so one command id never has two waiters at once.
    request_lock: TokioMutex<()>,
}

impl Inner {
    fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
        // Drop every waiter so its `recv` resolves to an error immediately.
        self.pending.lock().expect("pending lock poisoned").clear();
    }
}

/// A live Luci connection. Cheap to clone (shares one `Arc<Inner>`), though the
/// poller keeps a single owner per device.
#[derive(Clone)]
pub struct LuciClient {
    inner: Arc<Inner>,
}

impl LuciClient {
    /// Connect, complete the TLS handshake, subscribe to async events, and start
    /// the reader task. The returned receiver yields device pushes; drop it to
    /// stop caring about them (the connection stays up).
    pub async fn connect(ip: &str) -> AppResult<(Self, mpsc::Receiver<LuciEvent>)> {
        let connector = tls::connector()?;
        let addr = format!("{ip}:{LUCI_PORT}");

        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
            .await
            .map_err(|_| AppError::Device(format!("{ip} did not accept a Luci connection in time.")))?
            .map_err(|e| AppError::Device(format!("{ip} is unreachable on port {LUCI_PORT}: {e}")))?;
        // Nagle off: Luci frames are small and latency-sensitive.
        let _ = tcp.set_nodelay(true);

        let server_name = rustls_pki_types::ServerName::try_from(ip.to_string())
            .map_err(|e| AppError::Device(format!("{ip} is not a valid TLS server name: {e}")))?;
        let stream = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(server_name, tcp))
            .await
            .map_err(|_| AppError::Device(format!("{ip} did not complete the TLS handshake in time.")))?
            .map_err(|e| AppError::Device(format!("{ip} rejected the Luci TLS handshake: {e}")))?;

        let (read, write) = tokio::io::split(stream);
        let inner = Arc::new(Inner {
            ip: ip.to_string(),
            write: TokioMutex::new(write),
            pending: StdMutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            request_lock: TokioMutex::new(()),
        });

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        tokio::spawn(reader_loop(inner.clone(), read, event_tx));

        let client = Self { inner };
        // Subscribe to state pushes, exactly as the SDK does on connect.
        client
            .request(MessageBox::RegAsyncEvents.id(), MessageType::Write, "")
            .await
            .map_err(|e| AppError::Device(format!("{ip} refused REG_ASYNC_EVENTS: {e}")))?;

        Ok((client, event_rx))
    }

    /// The device this client is connected to.
    pub fn ip(&self) -> &str {
        &self.inner.ip
    }

    /// Send a frame and return the correlated reply's payload, erroring if the
    /// device answers `status != 1` or does not answer within
    /// [`REQUEST_TIMEOUT`].
    pub async fn request(&self, command: u16, mtype: MessageType, payload: &str) -> AppResult<String> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(AppError::Device(format!("{} is disconnected.", self.inner.ip)));
        }

        // Serialize: one in-flight request means one waiter per command id.
        let _guard = self.inner.request_lock.lock().await;

        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .expect("pending lock poisoned")
            .insert(command, reply_tx);

        let frame = codec::encode(command, mtype.id(), payload);
        if let Err(e) = self.write_all(&frame).await {
            self.inner.pending.lock().expect("pending lock poisoned").remove(&command);
            return Err(e);
        }

        match tokio::time::timeout(REQUEST_TIMEOUT, reply_rx).await {
            Ok(Ok(reply)) => {
                if reply.status == 1 {
                    Ok(reply.payload)
                } else {
                    Err(AppError::Device(format!(
                        "{} rejected command {command} (status {}).",
                        self.inner.ip, reply.status
                    )))
                }
            }
            // The reader dropped the sender: the connection is gone.
            Ok(Err(_)) => Err(AppError::Device(format!("{} closed the connection.", self.inner.ip))),
            Err(_) => {
                self.inner.pending.lock().expect("pending lock poisoned").remove(&command);
                Err(AppError::Device(format!(
                    "{} did not answer command {command} within {}s.",
                    self.inner.ip,
                    REQUEST_TIMEOUT.as_secs()
                )))
            }
        }
    }

    /// READ a `MessageBox` with an empty payload.
    pub async fn read(&self, mb: MessageBox) -> AppResult<String> {
        self.request(mb.id(), MessageType::Read, "").await
    }

    /// WRITE a `MessageBox` with a payload.
    pub async fn write(&self, mb: MessageBox, payload: &str) -> AppResult<String> {
        self.request(mb.id(), MessageType::Write, payload).await
    }

    /// WRITE without waiting for a correlated reply.
    ///
    /// LP10 firmware (`AR241CE_9243.16.2`) applies `VOLUME(64)` / `Mute_Unmute(63)`
    /// writes but sends **no reply frame** for them (verified live: the value
    /// takes effect, yet a `request` on cmd 64/63 always times out). Waiting would
    /// stall every volume tick for the full `REQUEST_TIMEOUT`, so these go out
    /// fire-and-forget — the poller confirms the new value a cycle later.
    pub async fn write_oneway(&self, mb: MessageBox, payload: &str) -> AppResult<()> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(AppError::Device(format!("{} is disconnected.", self.inner.ip)));
        }
        let frame = codec::encode(mb.id(), MessageType::Write.id(), payload);
        self.write_all(&frame).await
    }

    async fn write_all(&self, bytes: &[u8]) -> AppResult<()> {
        let mut write = self.inner.write.lock().await;
        write
            .write_all(bytes)
            .await
            .map_err(|e| AppError::Device(format!("{}: write failed: {e}", self.inner.ip)))?;
        write
            .flush()
            .await
            .map_err(|e| AppError::Device(format!("{}: flush failed: {e}", self.inner.ip)))
    }
}

/// The single reader task: decode frames, route correlated replies to their
/// waiters and push everything else to the event channel. Ends on EOF or error,
/// marking the connection closed.
async fn reader_loop(
    inner: Arc<Inner>,
    mut read: ReadHalf<TlsStream<TcpStream>>,
    event_tx: mpsc::Sender<LuciEvent>,
) {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];

    loop {
        let n = match read.read(&mut chunk).await {
            Ok(0) => break, // clean EOF
            Ok(n) => n,
            Err(e) => {
                log::debug!("{}: Luci read error: {e}", inner.ip);
                break;
            }
        };
        buf.extend_from_slice(&chunk[..n]);

        for frame in codec::drain_frames(&mut buf) {
            // A pending waiter for this command id claims the frame.
            let waiter = inner
                .pending
                .lock()
                .expect("pending lock poisoned")
                .remove(&frame.command);
            if let Some(tx) = waiter {
                let _ = tx.send(frame);
                continue;
            }
            // Otherwise it is a device push.
            match MessageBox::try_from(frame.command) {
                Ok(mb) => {
                    if event_tx.try_send((mb, frame.status, frame.payload)).is_err() {
                        log::trace!("{}: dropped a Luci push ({mb:?}) — channel full/closed", inner.ip);
                    }
                }
                Err(raw) => log::trace!("{}: unknown Luci push command {raw}", inner.ip),
            }
        }
    }

    inner.mark_closed();
}
