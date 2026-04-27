//! QUIC transport (Phase 2 spike).
//!
//! Each `Frame` variant maps to one of three transport classes (see PLAN.md §13):
//!
//! - **Unreliable QUIC datagrams** for `PointerAbs`, `Scroll`, `Heartbeat`.
//!   Drop on overflow; loss is fine.
//! - **Reliable bidirectional stream "input"** for `MouseButton`, `Key`. A
//!   dropped key-up causes stuck modifiers; a dropped click is a UX bug.
//! - **Reliable bidirectional stream "control"** for `LayoutUpdate`, `RttProbe`,
//!   `RttReply`. Separate from input so a slow input stream can't HoL the RTT
//!   probe.
//!
//! Streams are length-framed `[u32 length][bincode(WireFrame)]`; datagrams are
//! the bincode payload directly (QUIC datagrams are atomic, no length prefix).
//!
//! Cert handling for the spike:
//!
//! - On first `Endpoint::server` we mint a fresh self-signed cert via `rcgen`.
//! - Server accepts any client (no client auth).
//! - Client by default trusts any server cert (logs a WARN); pinned mode
//!   (`expected_fingerprint: Some([u8; 32])`) checks SHA-256 of the leaf.
//!
//! Phase 4 will swap the trust-any default for pinned-only; for now the GUI
//! drives pairing manually.

use anyhow::{anyhow, Context, Result};
use mousefly_core::Frame;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const HEALTH_INTERVAL: Duration = Duration::from_secs(1);
const HEALTH_WINDOW: usize = 256;
const OUTBOUND_BUF: usize = 1024;
const INBOUND_BUF: usize = 1024;
const MAX_FRAME_BYTES: usize = 1024 * 1024;
const ALPN: &[u8] = b"mousefly/0";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WireFrame {
    send_ts_ns: u64,
    frame: Frame,
}

/// One inbound input event with the (best-effort) one-way delay attached.
/// `one_way_delay_ns` is `None` until the first RTT probe round-trip completes.
#[derive(Debug, Clone)]
pub struct InboundFrame {
    pub frame: Frame,
    pub one_way_delay_ns: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LinkHealth {
    pub latency_p50_us: u32,
    pub latency_p99_us: u32,
    pub events_per_sec: u32,
    /// Estimated `remote_clock - local_clock` in nanoseconds. 0 until calibrated.
    pub clock_offset_ns: i64,
}

/// Owned handle on a live link. Drop to close.
pub struct Link {
    pub outbound: mpsc::Sender<Frame>,
    pub inbound: mpsc::Receiver<InboundFrame>,
    pub health: watch::Receiver<LinkHealth>,
    pub remote_addr: std::net::SocketAddr,
}

/// Wrapper around a `quinn::Endpoint`, holding the optional self-signed cert
/// bytes (server side) so callers can read the fingerprint to drive pairing.
pub struct Endpoint {
    inner: quinn::Endpoint,
    cert_der: Option<Vec<u8>>,
}

impl Endpoint {
    /// Bind a server endpoint and install a freshly-minted self-signed cert.
    pub fn server(bind_addr: &str) -> Result<Self> {
        install_default_crypto_provider();
        let (cert_der, key_der) = generate_self_signed()?;
        let server_config = build_server_config(cert_der.clone(), key_der)?;
        let addr: SocketAddr = bind_addr
            .parse()
            .with_context(|| format!("parsing bind addr {bind_addr}"))?;
        let endpoint = quinn::Endpoint::server(server_config, addr).context("quinn server bind")?;
        Ok(Self {
            inner: endpoint,
            cert_der: Some(cert_der),
        })
    }

    /// Bind a client endpoint (ephemeral UDP port). Per-connection trust is
    /// set up at `connect()` time so the same endpoint can dial multiple peers
    /// with different pinning rules later.
    ///
    /// Defaults to IPv4. Use [`Endpoint::client_for`] when the target address
    /// family is known (the bind family must match the peer's family on
    /// macOS / Windows; IPv6-only sockets reject IPv4 destinations by default).
    pub fn client() -> Result<Self> {
        Self::client_bind("0.0.0.0:0")
    }

    /// Bind a client endpoint matching `target`'s address family.
    pub fn client_for(target: SocketAddr) -> Result<Self> {
        let bind = match target {
            SocketAddr::V4(_) => "0.0.0.0:0",
            SocketAddr::V6(_) => "[::]:0",
        };
        Self::client_bind(bind)
    }

    fn client_bind(bind: &str) -> Result<Self> {
        install_default_crypto_provider();
        let bind_addr: SocketAddr = bind
            .parse()
            .with_context(|| format!("parsing client bind addr {bind}"))?;
        let endpoint = quinn::Endpoint::client(bind_addr).context("quinn client bind")?;
        Ok(Self {
            inner: endpoint,
            cert_der: None,
        })
    }

    /// DER bytes of the server's self-signed leaf cert. Empty for clients.
    pub fn cert_der(&self) -> &[u8] {
        self.cert_der.as_deref().unwrap_or(&[])
    }

    /// SHA-256 of `cert_der`. All-zero for clients (no cert).
    pub fn fingerprint(&self) -> [u8; 32] {
        match &self.cert_der {
            Some(der) => cert_fingerprint(der),
            None => [0u8; 32],
        }
    }
}

impl Endpoint {
    /// Local UDP port the QUIC endpoint is bound to. Useful for advertising
    /// the chosen pairing port via mDNS when we let the OS pick (port 0).
    pub fn local_port(&self) -> Result<u16> {
        Ok(self.inner.local_addr()?.port())
    }
}

/// Accept one incoming connection and one bidirectional stream — the raw
/// halves go to the SPAKE2 pairing handshake. Caller is responsible for
/// dropping the streams when done; the connection auto-closes on drop.
pub async fn pair_serve(endpoint: &Endpoint) -> Result<(quinn::SendStream, quinn::RecvStream)> {
    let incoming = endpoint
        .inner
        .accept()
        .await
        .ok_or_else(|| anyhow!("pair endpoint closed before any connection"))?;
    let conn = incoming.await.context("pair quic handshake (server)")?;
    info!(peer = %conn.remote_address(), "pair: peer connected");
    let (send, recv) = conn.accept_bi().await.context("pair: accept_bi")?;
    Ok((send, recv))
}

/// Dial `addr` (no cert pinning — pairing is exactly when we don't yet know
/// the peer's fingerprint) and open one bidirectional stream for the SPAKE2
/// handshake. Binds an ephemeral client endpoint matching the target's address
/// family so v6 peers work without manual configuration.
///
/// `addr` accepts both IPv4 (`192.168.1.5:7878`) and IPv6 (`[fe80::1]:7878`)
/// forms. Link-local IPv6 (`fe80::/10`) without a zone identifier won't route;
/// callers should prefer routable addresses where mDNS gives both.
pub async fn pair_connect(addr: &str) -> Result<(quinn::SendStream, quinn::RecvStream)> {
    let remote: SocketAddr = addr
        .parse()
        .with_context(|| format!("parsing pair peer addr {addr}"))?;
    let endpoint = Endpoint::client_for(remote)?;
    let client_config = build_client_config(None)?;
    let connecting = endpoint
        .inner
        .connect_with(client_config, remote, "mousefly")
        .context("pair quic connect_with")?;
    let conn = connecting.await.context("pair quic handshake (client)")?;
    info!(peer = %addr, "pair: connected");
    let (send, recv) = conn.open_bi().await.context("pair: open_bi")?;
    // Push a zero-length write so the peer's accept_bi() returns immediately.
    let mut send = send;
    send.write_all(&[]).await.ok();
    Ok((send, recv))
}

/// SHA-256 of a DER-encoded certificate.
pub fn cert_fingerprint(cert_der: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(cert_der);
    hasher.finalize().into()
}

/// Convenience wrapper: bind a fresh server endpoint and accept the first peer.
/// Mousefly-app uses this so the Phase 0 call sites keep compiling.
pub async fn serve(addr: &str) -> Result<Link> {
    let endpoint = Endpoint::server(addr)?;
    serve_on(&endpoint).await
}

/// Convenience wrapper: dial `addr` from a fresh client endpoint, no pinning.
pub async fn connect(addr: &str) -> Result<Link> {
    let parsed: SocketAddr = addr
        .parse()
        .with_context(|| format!("parsing peer addr {addr}"))?;
    let endpoint = Endpoint::client_for(parsed)?;
    connect_on(&endpoint, addr, None).await
}

/// Connect with an expected SHA-256 leaf-cert fingerprint. Reject mismatches.
pub async fn connect_pinned(addr: &str, expected_fingerprint: [u8; 32]) -> Result<Link> {
    let parsed: SocketAddr = addr
        .parse()
        .with_context(|| format!("parsing peer addr {addr}"))?;
    let endpoint = Endpoint::client_for(parsed)?;
    connect_on(&endpoint, addr, Some(expected_fingerprint)).await
}

/// Accept the next incoming connection on `endpoint`. Phase 2 is single-peer.
pub async fn serve_on(endpoint: &Endpoint) -> Result<Link> {
    let local = endpoint
        .inner
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());
    info!(addr = %local, "quic listening");
    let incoming = endpoint
        .inner
        .accept()
        .await
        .ok_or_else(|| anyhow!("endpoint closed before any connection"))?;
    let conn = incoming.await.context("quic handshake (server)")?;
    info!(peer = %conn.remote_address(), "peer connected");
    spawn_link_server(conn).await
}

/// Connect on `endpoint`. With `expected_fingerprint = Some(fp)` the leaf cert
/// SHA-256 must match. With `None` we trust any server (logs WARN).
pub async fn connect_on(
    endpoint: &Endpoint,
    addr: &str,
    expected_fingerprint: Option<[u8; 32]>,
) -> Result<Link> {
    let remote: SocketAddr = addr
        .parse()
        .with_context(|| format!("parsing peer addr {addr}"))?;
    let client_config = build_client_config(expected_fingerprint)?;
    let connecting = endpoint
        .inner
        .connect_with(client_config, remote, "mousefly")
        .context("quic connect_with")?;
    let conn = connecting.await.context("quic handshake (client)")?;
    info!(addr = %addr, "connected");
    spawn_link_client(conn).await
}

async fn spawn_link_client(conn: quinn::Connection) -> Result<Link> {
    // The dialer opens both stream-classes. Server accepts them in the same
    // order. Single-peer for Phase 2, so we don't bother with a stream-name
    // handshake — order is the contract.
    let input = conn.open_bi().await.context("open input stream")?;
    let control = conn.open_bi().await.context("open control stream")?;
    // Push one byte on each so the server's `accept_bi()` returns immediately.
    let (mut input_tx, input_rx) = input;
    let (mut control_tx, control_rx) = control;
    input_tx.write_all(&[0u8; 0]).await.ok();
    control_tx.write_all(&[0u8; 0]).await.ok();
    Ok(spawn_link(
        conn,
        (input_tx, input_rx),
        (control_tx, control_rx),
    ))
}

async fn spawn_link_server(conn: quinn::Connection) -> Result<Link> {
    let input = conn.accept_bi().await.context("accept input stream")?;
    let control = conn.accept_bi().await.context("accept control stream")?;
    Ok(spawn_link(conn, input, control))
}

fn spawn_link(
    conn: quinn::Connection,
    (input_tx, input_rx): (quinn::SendStream, quinn::RecvStream),
    (control_tx, control_rx): (quinn::SendStream, quinn::RecvStream),
) -> Link {
    let (outbound_tx, outbound_rx) = mpsc::channel::<Frame>(OUTBOUND_BUF);
    let (inbound_tx, inbound_rx) = mpsc::channel::<InboundFrame>(INBOUND_BUF);
    let (health_tx, health_rx) = watch::channel(LinkHealth::default());

    // Per-class outbound channels. Each stream-writer task owns its stream.
    let (input_out_tx, input_out_rx) = mpsc::channel::<WireFrame>(OUTBOUND_BUF);
    let (control_out_tx, control_out_rx) = mpsc::channel::<WireFrame>(OUTBOUND_BUF);

    // Shared clock-offset for one-way-delay calculations on the inbound side.
    let clock_offset = Arc::new(std::sync::atomic::AtomicI64::new(0));

    // Outbound dispatcher: pick datagram vs input vs control per Frame variant.
    {
        let conn = conn.clone();
        let input_out_tx = input_out_tx.clone();
        let control_out_tx_dispatch = control_out_tx.clone();
        tokio::spawn(async move {
            let mut rx = outbound_rx;
            while let Some(frame) = rx.recv().await {
                let wire = WireFrame {
                    send_ts_ns: now_ns(),
                    frame,
                };
                match transport_class(&wire.frame) {
                    Class::Datagram => {
                        let payload = match bincode::serialize(&wire) {
                            Ok(p) => p,
                            Err(e) => {
                                warn!("bincode datagram serialize: {e}");
                                continue;
                            }
                        };
                        // Quinn's send_datagram drops on overflow / oversize —
                        // exactly the semantics we want for stale pointer deltas.
                        if let Err(e) = conn.send_datagram(payload.into()) {
                            debug!("datagram dropped: {e}");
                        }
                    }
                    Class::Input => {
                        if input_out_tx.send(wire).await.is_err() {
                            break;
                        }
                    }
                    Class::Control => {
                        if control_out_tx_dispatch.send(wire).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    // Stream writers (one per stream-class).
    spawn_stream_writer("input", input_tx, input_out_rx);
    spawn_stream_writer("control", control_tx, control_out_rx);

    // Periodic RTT probe — pushed onto control.
    {
        let control_out_tx = control_out_tx.clone();
        tokio::spawn(async move {
            let mut id: u32 = 0;
            let mut tick = tokio::time::interval(PROBE_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                id = id.wrapping_add(1);
                let probe = WireFrame {
                    send_ts_ns: now_ns(),
                    frame: Frame::RttProbe {
                        id,
                        t1_ns: now_ns(),
                    },
                };
                if control_out_tx.send(probe).await.is_err() {
                    break;
                }
            }
        });
    }

    // Bookkeeping shared between the three readers.
    let bookkeeping = Bookkeeping::new(inbound_tx.clone(), health_tx, clock_offset.clone());

    // Datagram reader: PointerAbs / Scroll / Heartbeat — no RTT here.
    {
        let conn = conn.clone();
        let bk = bookkeeping.clone();
        tokio::spawn(async move {
            if let Err(e) = read_datagrams(conn, bk).await {
                debug!("datagram reader ended: {e:#}");
            }
        });
    }

    // Input stream reader: MouseButton, Key.
    {
        let bk = bookkeeping.clone();
        tokio::spawn(async move {
            if let Err(e) = read_stream("input", input_rx, bk, /*is_control=*/ false).await {
                debug!("input reader ended: {e:#}");
            }
        });
    }

    // Control stream reader: LayoutUpdate, RttProbe, RttReply (+ probe replies).
    {
        let bk = bookkeeping.clone();
        let control_out_tx = control_out_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = read_control_stream(control_rx, bk, control_out_tx).await {
                debug!("control reader ended: {e:#}");
            }
        });
    }

    // Health publisher: 1 s tick, snapshots the rolling-window state.
    {
        let bk = bookkeeping;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(HEALTH_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if !bk.publish_health() {
                    break;
                }
            }
        });
    }

    // Connection-closed watchdog. When the QUIC connection dies, close the
    // inbound channel so the app loop sees EOF and exits.
    {
        let conn = conn.clone();
        let inbound_tx = inbound_tx;
        tokio::spawn(async move {
            let reason = conn.closed().await;
            info!("quic connection closed: {reason}");
            drop(inbound_tx);
        });
    }

    let remote_addr = conn.remote_address();

    Link {
        outbound: outbound_tx,
        inbound: inbound_rx,
        health: health_rx,
        remote_addr,
    }
}

#[derive(Clone)]
struct Bookkeeping {
    inbound: mpsc::Sender<InboundFrame>,
    health: Arc<watch::Sender<LinkHealth>>,
    clock_offset_ns: Arc<std::sync::atomic::AtomicI64>,
    samples: Arc<std::sync::Mutex<std::collections::VecDeque<i64>>>,
    events_in_window: Arc<std::sync::atomic::AtomicU32>,
}

impl Bookkeeping {
    fn new(
        inbound: mpsc::Sender<InboundFrame>,
        health: watch::Sender<LinkHealth>,
        clock_offset_ns: Arc<std::sync::atomic::AtomicI64>,
    ) -> Self {
        Self {
            inbound,
            health: Arc::new(health),
            clock_offset_ns,
            samples: Arc::new(std::sync::Mutex::new(
                std::collections::VecDeque::with_capacity(HEALTH_WINDOW),
            )),
            events_in_window: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    async fn deliver(&self, wire: WireFrame, recv_ts: u64) -> Result<()> {
        let offset = self
            .clock_offset_ns
            .load(std::sync::atomic::Ordering::Relaxed);
        let remote_send_local = wire.send_ts_ns as i64 + offset;
        let one_way = recv_ts as i64 - remote_send_local;
        {
            let mut buf = self.samples.lock().unwrap();
            push_sample(&mut buf, one_way);
        }
        self.events_in_window
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inbound
            .send(InboundFrame {
                frame: wire.frame,
                one_way_delay_ns: Some(one_way),
            })
            .await
            .map_err(|_| anyhow!("inbound channel closed"))
    }

    fn publish_health(&self) -> bool {
        if self.inbound.is_closed() {
            return false;
        }
        let (p50, p99) = {
            let buf = self.samples.lock().unwrap();
            (percentile_us(&buf, 50), percentile_us(&buf, 99))
        };
        let events = self
            .events_in_window
            .swap(0, std::sync::atomic::Ordering::Relaxed);
        let snapshot = LinkHealth {
            latency_p50_us: p50,
            latency_p99_us: p99,
            events_per_sec: events,
            clock_offset_ns: self
                .clock_offset_ns
                .load(std::sync::atomic::Ordering::Relaxed),
        };
        self.health.send(snapshot).is_ok()
    }
}

#[derive(Copy, Clone)]
enum Class {
    Datagram,
    Input,
    Control,
}

fn transport_class(frame: &Frame) -> Class {
    match frame {
        Frame::PointerAbs { .. }
        | Frame::PointerOnMonitor { .. }
        | Frame::Scroll { .. }
        | Frame::Heartbeat => Class::Datagram,
        Frame::MouseButton { .. } | Frame::Key { .. } => Class::Input,
        Frame::LayoutUpdate { .. }
        | Frame::RttProbe { .. }
        | Frame::RttReply { .. }
        | Frame::Clipboard { .. } => Class::Control,
    }
}

fn spawn_stream_writer(
    name: &'static str,
    mut tx: quinn::SendStream,
    mut rx: mpsc::Receiver<WireFrame>,
) {
    tokio::spawn(async move {
        while let Some(wire) = rx.recv().await {
            let payload = match bincode::serialize(&wire) {
                Ok(p) => p,
                Err(e) => {
                    warn!("{name}: bincode serialize: {e}");
                    continue;
                }
            };
            let len = match u32::try_from(payload.len()) {
                Ok(l) if (l as usize) <= MAX_FRAME_BYTES => l,
                _ => {
                    warn!("{name}: frame too large ({} bytes)", payload.len());
                    continue;
                }
            };
            if tx.write_all(&len.to_be_bytes()).await.is_err() {
                break;
            }
            if tx.write_all(&payload).await.is_err() {
                break;
            }
        }
        let _ = tx.finish();
    });
}

async fn read_datagrams(conn: quinn::Connection, bk: Bookkeeping) -> Result<()> {
    loop {
        let bytes = conn.read_datagram().await.context("read_datagram")?;
        let recv_ts = now_ns();
        let wire: WireFrame = match bincode::deserialize(&bytes) {
            Ok(w) => w,
            Err(e) => {
                warn!("datagram bincode: {e}");
                continue;
            }
        };
        if bk.deliver(wire, recv_ts).await.is_err() {
            return Ok(());
        }
    }
}

async fn read_stream(
    name: &'static str,
    mut rx: quinn::RecvStream,
    bk: Bookkeeping,
    is_control: bool,
) -> Result<()> {
    loop {
        let wire = match read_wire_frame(&mut rx).await? {
            Some(w) => w,
            None => return Ok(()),
        };
        let recv_ts = now_ns();
        // Non-control streams should never carry RTT frames; defensive ignore.
        if !is_control {
            if let Frame::RttProbe { .. } | Frame::RttReply { .. } = wire.frame {
                debug!("{name}: unexpected RTT frame on non-control stream");
                continue;
            }
        }
        if bk.deliver(wire, recv_ts).await.is_err() {
            return Ok(());
        }
    }
}

async fn read_control_stream(
    mut rx: quinn::RecvStream,
    bk: Bookkeeping,
    control_out: mpsc::Sender<WireFrame>,
) -> Result<()> {
    loop {
        let wire = match read_wire_frame(&mut rx).await? {
            Some(w) => w,
            None => return Ok(()),
        };
        let recv_ts = now_ns();
        match wire.frame {
            Frame::RttProbe { id, t1_ns } => {
                let reply = WireFrame {
                    send_ts_ns: now_ns(),
                    frame: Frame::RttReply {
                        id,
                        t1_ns,
                        t2_ns: recv_ts,
                        t3_ns: now_ns(),
                    },
                };
                if control_out.send(reply).await.is_err() {
                    return Ok(());
                }
            }
            Frame::RttReply {
                id: _,
                t1_ns,
                t2_ns,
                t3_ns,
            } => {
                let t4_ns = recv_ts;
                let d1 = t2_ns as i64 - t1_ns as i64;
                let d2 = t3_ns as i64 - t4_ns as i64;
                let offset = (d1 + d2) / 2;
                bk.clock_offset_ns
                    .store(offset, std::sync::atomic::Ordering::Relaxed);
                debug!(clock_offset_ns = offset, "rtt calibration updated");
            }
            _ => {
                if bk.deliver(wire, recv_ts).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
}

async fn read_wire_frame(rx: &mut quinn::RecvStream) -> Result<Option<WireFrame>> {
    let mut len_buf = [0u8; 4];
    match rx.read_exact(&mut len_buf).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(anyhow!("stream read len: {e}")),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(anyhow!("oversized frame: {len} bytes"));
    }
    let mut payload = vec![0u8; len];
    rx.read_exact(&mut payload)
        .await
        .map_err(|e| anyhow!("stream read body: {e}"))?;
    let wire: WireFrame = bincode::deserialize(&payload).context("bincode deserialize")?;
    Ok(Some(wire))
}

fn push_sample(buf: &mut std::collections::VecDeque<i64>, sample_ns: i64) {
    if buf.len() == HEALTH_WINDOW {
        buf.pop_front();
    }
    buf.push_back(sample_ns);
}

fn percentile_us(buf: &std::collections::VecDeque<i64>, p: u8) -> u32 {
    if buf.is_empty() {
        return 0;
    }
    let mut v: Vec<i64> = buf.iter().copied().collect();
    v.sort_unstable();
    let idx = ((p as usize * (v.len() - 1)) + 50) / 100;
    let ns = v[idx].max(0) as u64;
    (ns / 1_000) as u32
}

fn now_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = *EPOCH.get_or_init(Instant::now);
    Instant::now().duration_since(epoch).as_nanos() as u64
}

// ---- TLS / cert plumbing -------------------------------------------------

fn install_default_crypto_provider() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn generate_self_signed() -> Result<(Vec<u8>, Vec<u8>)> {
    let cert =
        rcgen::generate_simple_self_signed(vec!["mousefly".into()]).context("rcgen self-signed")?;
    let cert_der = cert.cert.der().to_vec();
    let key_der = cert.key_pair.serialize_der();
    Ok((cert_der, key_der))
}

fn build_server_config(cert_der: Vec<u8>, key_der: Vec<u8>) -> Result<quinn::ServerConfig> {
    let cert = CertificateDer::from(cert_der);
    let key = PrivateKeyDer::try_from(key_der).map_err(|e| anyhow!("private key parse: {e}"))?;
    // Server accepts any client (no client auth in Phase 2). We use an empty
    // root store + `WebPkiClientVerifier::no_client_auth()` rather than the
    // do-nothing custom verifier — it's simpler and matches our intent.
    let mut tls = rustls::ServerConfig::builder()
        .with_client_cert_verifier(WebPkiClientVerifier::no_client_auth())
        .with_single_cert(vec![cert], key)
        .map_err(|e| anyhow!("rustls server cfg: {e}"))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let quic_tls = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .map_err(|e| anyhow!("quic server tls: {e}"))?;
    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_tls));
    let mut transport = quinn::TransportConfig::default();
    // Datagrams are mandatory for our pointer path.
    transport.datagram_send_buffer_size(1 << 20);
    transport.datagram_receive_buffer_size(Some(1 << 20));
    server_config.transport_config(Arc::new(transport));
    Ok(server_config)
}

fn build_client_config(expected_fingerprint: Option<[u8; 32]>) -> Result<quinn::ClientConfig> {
    if expected_fingerprint.is_none() {
        warn!("connecting with trust-any-server cert; pin a fingerprint in production");
    }
    let verifier: Arc<dyn ServerCertVerifier> = match expected_fingerprint {
        Some(fp) => Arc::new(PinnedFingerprintVerifier { expected: fp }),
        None => Arc::new(SkipServerVerification),
    };
    let mut tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let quic_tls = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| anyhow!("quic client tls: {e}"))?;
    let mut client_config = quinn::ClientConfig::new(Arc::new(quic_tls));
    let mut transport = quinn::TransportConfig::default();
    transport.datagram_send_buffer_size(1 << 20);
    transport.datagram_receive_buffer_size(Some(1 << 20));
    // Keepalive doubles as our anti-Wi-Fi-power-save heartbeat at the QUIC
    // level — independent of the app-level Heartbeat frame.
    transport.keep_alive_interval(Some(Duration::from_millis(750)));
    client_config.transport_config(Arc::new(transport));
    Ok(client_config)
}

#[derive(Debug)]
struct SkipServerVerification;

impl ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        all_schemes()
    }
}

#[derive(Debug)]
struct PinnedFingerprintVerifier {
    expected: [u8; 32],
}

impl ServerCertVerifier for PinnedFingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let actual = cert_fingerprint(end_entity.as_ref());
        if actual == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "cert fingerprint mismatch: got {} expected {}",
                hex(&actual),
                hex(&self.expected)
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        all_schemes()
    }
}

fn all_schemes() -> Vec<SignatureScheme> {
    vec![
        SignatureScheme::RSA_PKCS1_SHA256,
        SignatureScheme::ECDSA_NISTP256_SHA256,
        SignatureScheme::ED25519,
        SignatureScheme::RSA_PSS_SHA256,
        SignatureScheme::RSA_PSS_SHA384,
        SignatureScheme::RSA_PSS_SHA512,
        SignatureScheme::ECDSA_NISTP384_SHA384,
        SignatureScheme::ECDSA_NISTP521_SHA512,
        SignatureScheme::RSA_PKCS1_SHA384,
        SignatureScheme::RSA_PKCS1_SHA512,
    ]
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use mousefly_core::{button, Frame};

    #[tokio::test]
    async fn roundtrip_datagram_and_stream() {
        let server = Endpoint::server("127.0.0.1:0").unwrap();
        let bind = server.inner.local_addr().unwrap();
        let fp = server.fingerprint();

        let server_link = tokio::spawn(async move { serve_on(&server).await.unwrap() });

        let client_ep = Endpoint::client().unwrap();
        let client = connect_on(&client_ep, &bind.to_string(), Some(fp))
            .await
            .unwrap();

        let mut server = server_link.await.unwrap();

        // Datagram path: PointerAbs.
        client
            .outbound
            .send(Frame::PointerAbs {
                x: 1.0,
                y: 2.0,
                dx: 0.5,
                dy: 0.5,
                buttons: 0,
            })
            .await
            .unwrap();
        let got = tokio::time::timeout(Duration::from_secs(2), server.inbound.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            got.frame,
            Frame::PointerAbs { x, y, .. } if (x - 1.0).abs() < 1e-3 && (y - 2.0).abs() < 1e-3
        ));

        // Reliable input stream: Key.
        client
            .outbound
            .send(Frame::Key {
                code: 42,
                down: true,
                modifiers: button::LEFT,
            })
            .await
            .unwrap();
        let got = tokio::time::timeout(Duration::from_secs(2), server.inbound.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            got.frame,
            Frame::Key {
                code: 42,
                down: true,
                ..
            }
        ));
    }

    #[test]
    fn fingerprint_is_sha256() {
        let der = b"hello";
        let fp = cert_fingerprint(der);
        let mut h = Sha256::new();
        h.update(der);
        let want: [u8; 32] = h.finalize().into();
        assert_eq!(fp, want);
    }
}
