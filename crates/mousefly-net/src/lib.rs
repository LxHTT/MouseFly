//! TCP transport for Phase 0.
//!
//! Wire format: `[u32 length][bincode(WireFrame)]` over a TCP stream. Each
//! `WireFrame` carries a monotonic `send_ts_ns` taken at flush time. The
//! receiver pairs that with its own `recv_ts_ns` to compute one-way delay,
//! after subtracting a clock-offset estimate produced by a 4-timestamp
//! NTP-style RTT probe (`Frame::RttProbe` / `Frame::RttReply`) running on a
//! 2 s cadence in the background.
//!
//! This is a deliberately small spike. Phase 2 replaces it with QUIC, mDNS
//! discovery, SPAKE2 pairing, and ed25519-pinned mutual TLS.

use anyhow::{anyhow, Context, Result};
use mousefly_core::Frame;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const HEALTH_INTERVAL: Duration = Duration::from_secs(1);
const HEALTH_WINDOW: usize = 256;
const OUTBOUND_BUF: usize = 1024;
const INBOUND_BUF: usize = 1024;

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
}

/// Bind on `addr` and accept the first incoming connection. Phase 0 is single-peer.
pub async fn serve(addr: &str) -> Result<Link> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    info!(%addr, "listening");
    let (stream, peer) = listener.accept().await.context("accepting peer")?;
    info!(%peer, "peer connected");
    stream.set_nodelay(true).ok();
    Ok(spawn_link(stream))
}

/// Connect to the peer at `addr`. Phase 0 is single-peer.
pub async fn connect(addr: &str) -> Result<Link> {
    let stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("connecting to {addr}"))?;
    info!(%addr, "connected");
    stream.set_nodelay(true).ok();
    Ok(spawn_link(stream))
}

fn spawn_link(stream: TcpStream) -> Link {
    let (outbound_tx, outbound_rx) = mpsc::channel::<Frame>(OUTBOUND_BUF);
    let (inbound_tx, inbound_rx) = mpsc::channel::<InboundFrame>(INBOUND_BUF);
    let (health_tx, health_rx) = watch::channel(LinkHealth::default());

    let (read_half, write_half) = stream.into_split();
    let writer = SharedWriter::new(write_half);

    // Spawn the periodic RTT probe.
    {
        let writer = writer.clone();
        tokio::spawn(async move {
            let mut id: u32 = 0;
            let mut tick = tokio::time::interval(PROBE_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                id = id.wrapping_add(1);
                let probe = Frame::RttProbe {
                    id,
                    t1_ns: now_ns(),
                };
                if writer.send(probe).await.is_err() {
                    break;
                }
            }
        });
    }

    // Spawn the outbound pump (caller-supplied frames).
    {
        let writer = writer.clone();
        tokio::spawn(async move {
            let mut rx = outbound_rx;
            while let Some(frame) = rx.recv().await {
                if writer.send(frame).await.is_err() {
                    break;
                }
            }
        });
    }

    // Spawn the reader: dispatches inbound frames, replies to probes, samples
    // delay, computes rolling p50/p99, publishes link health every second.
    {
        let writer = writer.clone();
        tokio::spawn(async move {
            if let Err(e) = read_loop(read_half, writer, inbound_tx, health_tx).await {
                warn!("read loop ended: {e:#}");
            }
        });
    }

    Link {
        outbound: outbound_tx,
        inbound: inbound_rx,
        health: health_rx,
    }
}

async fn read_loop(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    writer: SharedWriter,
    inbound: mpsc::Sender<InboundFrame>,
    health: watch::Sender<LinkHealth>,
) -> Result<()> {
    let mut clock_offset_ns: i64 = 0;
    let mut samples: std::collections::VecDeque<i64> =
        std::collections::VecDeque::with_capacity(HEALTH_WINDOW);
    let mut last_health = Instant::now();
    let mut events_in_window: u32 = 0;

    loop {
        let wire = match read_wire_frame(&mut reader).await? {
            Some(w) => w,
            None => return Ok(()),
        };
        let recv_ts = now_ns();

        match wire.frame {
            Frame::RttProbe { id, t1_ns } => {
                let reply = Frame::RttReply {
                    id,
                    t1_ns,
                    t2_ns: recv_ts,
                    t3_ns: now_ns(),
                };
                let _ = writer.send(reply).await;
            }
            Frame::RttReply {
                id: _,
                t1_ns,
                t2_ns,
                t3_ns,
            } => {
                let t4_ns = recv_ts;
                // NTP four-timestamp:
                //   offset = ((t2 - t1) + (t3 - t4)) / 2
                let d1 = t2_ns as i64 - t1_ns as i64;
                let d2 = t3_ns as i64 - t4_ns as i64;
                clock_offset_ns = (d1 + d2) / 2;
                debug!(clock_offset_ns, "rtt calibration updated");
            }
            ref f => {
                // One-way delay = local_recv - (remote_send + offset)
                let remote_send_local = wire.send_ts_ns as i64 + clock_offset_ns;
                let one_way = recv_ts as i64 - remote_send_local;
                push_sample(&mut samples, one_way);
                events_in_window = events_in_window.saturating_add(1);
                let frame = f.clone();
                if inbound
                    .send(InboundFrame {
                        frame,
                        one_way_delay_ns: Some(one_way),
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }
            }
        }

        if last_health.elapsed() >= HEALTH_INTERVAL {
            let snapshot = LinkHealth {
                latency_p50_us: percentile_us(&samples, 50),
                latency_p99_us: percentile_us(&samples, 99),
                events_per_sec: events_in_window,
                clock_offset_ns,
            };
            let _ = health.send(snapshot);
            events_in_window = 0;
            last_health = Instant::now();
        }
    }
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

#[derive(Clone)]
struct SharedWriter {
    tx: mpsc::Sender<Vec<u8>>,
}

impl SharedWriter {
    fn new(mut writer: tokio::net::tcp::OwnedWriteHalf) -> Self {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(OUTBOUND_BUF);
        tokio::spawn(async move {
            while let Some(buf) = rx.recv().await {
                if writer.write_all(&buf).await.is_err() {
                    break;
                }
            }
        });
        Self { tx }
    }

    async fn send(&self, frame: Frame) -> Result<()> {
        let wire = WireFrame {
            send_ts_ns: now_ns(),
            frame,
        };
        let payload = bincode::serialize(&wire).context("bincode serialize")?;
        let len = u32::try_from(payload.len()).map_err(|_| anyhow!("frame too large"))?;
        let mut buf = Vec::with_capacity(4 + payload.len());
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&payload);
        self.tx
            .send(buf)
            .await
            .map_err(|_| anyhow!("writer task gone"))
    }
}

async fn read_wire_frame(reader: &mut tokio::net::tcp::OwnedReadHalf) -> Result<Option<WireFrame>> {
    let mut len_buf = [0u8; 4];
    if let Err(e) = reader.read_exact(&mut len_buf).await {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            return Ok(None);
        }
        return Err(e.into());
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 1024 * 1024 {
        return Err(anyhow!("oversized frame: {len} bytes"));
    }
    let mut payload = vec![0u8; len];
    reader
        .read_exact(&mut payload)
        .await
        .context("reading frame payload")?;
    let wire: WireFrame = bincode::deserialize(&payload).context("bincode deserialize")?;
    Ok(Some(wire))
}

fn now_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = *EPOCH.get_or_init(Instant::now);
    Instant::now().duration_since(epoch).as_nanos() as u64
}
