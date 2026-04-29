//! mDNS / DNS-SD peer discovery on the LAN.
//!
//! Service type: `_mousefly._udp.local.` (PLAN.md §7).
//!
//! TXT record:
//! - `fp` — 64-char hex SHA-256 fingerprint of the host's pinning cert.
//! - `id` — 64-char hex ed25519 public key. Empty until the host has
//!   generated a pairing identity.
//!
//! [`Advertiser`] publishes the local host. [`Browser`] watches the LAN and
//! exposes peers as both a snapshot and a `tokio::sync::broadcast` stream.
//! mdns-sd's own event channel is a `flume::Receiver`; we bridge it to a
//! broadcast channel so multiple frontend subscribers (Tauri events, Pinia
//! stores, tests) can share one daemon.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::debug;

const SERVICE_TYPE: &str = "_mousefly._udp.local.";
const TXT_FINGERPRINT: &str = "fp";
const TXT_HOST_ID: &str = "id";
const TXT_DATA_PORT: &str = "dp";
const EVENT_BUFFER: usize = 64;

/// Inputs for [`Advertiser::start`].
#[derive(Debug, Clone)]
pub struct AdvertiseConfig {
    /// Service instance label. The OS hostname is a sensible default; falls
    /// back to `"mousefly"` if the caller has nothing better.
    pub instance_name: String,
    /// Pair-handshake QUIC port (SRV record). The joiner dials this port for
    /// the SPAKE2 exchange.
    pub port: u16,
    /// Data-link QUIC port (TXT `dp`). The joiner dials this port for actual
    /// pointer / key forwarding once pairing succeeds.
    pub data_port: u16,
    /// 64-char hex SHA-256 of the host's pinning cert.
    pub fingerprint_hex: String,
    /// 64-char hex ed25519 public key. May be empty until pairing identity exists.
    pub host_id_hex: String,
}

/// Owns the registered service. Drop or call [`Advertiser::stop`] to retract it.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertiser {
    /// Register the service with the system mDNS responder.
    pub fn start(cfg: AdvertiseConfig) -> Result<Self> {
        let daemon = ServiceDaemon::new().context("creating mdns ServiceDaemon")?;

        let host_label = sanitize_hostname(&cfg.instance_name);
        // mdns-sd needs a hostname ending in `.local.`. We synthesize one from
        // the instance name; mdns-sd will fill in addresses for us when we pass
        // an empty addr set (the daemon enumerates non-loopback interfaces).
        let host_name = format!("{host_label}.local.");

        let mut properties: HashMap<String, String> = HashMap::new();
        properties.insert(TXT_FINGERPRINT.to_string(), cfg.fingerprint_hex.clone());
        properties.insert(TXT_HOST_ID.to_string(), cfg.host_id_hex.clone());
        properties.insert(TXT_DATA_PORT.to_string(), cfg.data_port.to_string());

        let no_addrs: &[IpAddr] = &[];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &cfg.instance_name,
            &host_name,
            no_addrs,
            cfg.port,
            properties,
        )
        .context("building mdns ServiceInfo")?
        .enable_addr_auto();

        let fullname = info.get_fullname().to_string();
        daemon.register(info).context("registering mdns service")?;

        debug!(service = %fullname, port = cfg.port, "mdns service registered");
        Ok(Self { daemon, fullname })
    }

    /// Unregister and shut the daemon down.
    pub fn stop(self) -> Result<()> {
        // unregister returns a flume::Receiver of UnregisterStatus; we drop it
        // because the shutdown below already tears the daemon down anyway.
        let _ = self.daemon.unregister(&self.fullname);
        self.daemon.shutdown().ok();
        Ok(())
    }
}

/// One peer seen on the LAN. `is_self` is set when the peer's fingerprint
/// matches the one passed to [`Browser::start`] — frontend should hide it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPeer {
    pub instance_name: String,
    pub addrs: Vec<IpAddr>,
    /// Pair-handshake port (SRV).
    pub port: u16,
    /// Data-link port (TXT `dp`). 0 if missing — joiner can fall back to
    /// the pair port or prompt the user.
    pub data_port: u16,
    pub fingerprint_hex: String,
    pub host_id_hex: String,
    pub is_self: bool,
}

/// Lifecycle event for a peer. Backed by `serde` so it can travel over Tauri
/// IPC to the Vue layer without re-shaping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerEvent {
    Added(DiscoveredPeer),
    Updated(DiscoveredPeer),
    Removed { instance_name: String },
}

type PeerMap = Arc<Mutex<HashMap<String, DiscoveredPeer>>>;

/// Watches `_mousefly._udp.local.` and forwards peer changes.
pub struct Browser {
    daemon: ServiceDaemon,
    peers: PeerMap,
    events_tx: broadcast::Sender<PeerEvent>,
}

impl Browser {
    /// Start browsing. `own_fingerprint_hex` is matched against discovered
    /// peers' `fp` TXT to set [`DiscoveredPeer::is_self`].
    pub fn start(own_fingerprint_hex: String) -> Result<Self> {
        let daemon = ServiceDaemon::new().context("creating mdns ServiceDaemon")?;
        let receiver = daemon
            .browse(SERVICE_TYPE)
            .context("starting mdns browse")?;

        let peers: PeerMap = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, _) = broadcast::channel(EVENT_BUFFER);

        // Bridge: read mdns-sd's flume channel on a blocking task, fan out to
        // the broadcast channel, and update the snapshot map.
        let peers_bg = peers.clone();
        let events_bg = events_tx.clone();
        tokio::task::spawn_blocking(move || {
            forward_events(receiver, peers_bg, events_bg, own_fingerprint_hex);
        });

        Ok(Self {
            daemon,
            peers,
            events_tx,
        })
    }

    /// New subscriber on the live event stream. Combine with [`Self::snapshot`]
    /// to build initial UI state without missing later updates.
    pub fn events(&self) -> broadcast::Receiver<PeerEvent> {
        self.events_tx.subscribe()
    }

    /// Current set of resolved peers.
    pub fn snapshot(&self) -> Vec<DiscoveredPeer> {
        self.peers
            .lock()
            .expect("peer map mutex poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// Stop browsing and shut the daemon down.
    pub fn stop(self) -> Result<()> {
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        self.daemon.shutdown().ok();
        Ok(())
    }
}

fn forward_events(
    receiver: flume::Receiver<ServiceEvent>,
    peers: PeerMap,
    events_tx: broadcast::Sender<PeerEvent>,
    own_fp: String,
) {
    while let Ok(event) = receiver.recv() {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                let peer = peer_from_service_info(&info, &own_fp);
                let key = peer.instance_name.clone();
                let was_present = {
                    let mut map = peers.lock().expect("peer map mutex poisoned");
                    let existed = map.contains_key(&key);
                    map.insert(key.clone(), peer.clone());
                    existed
                };
                let evt = if was_present {
                    PeerEvent::Updated(peer)
                } else {
                    PeerEvent::Added(peer)
                };
                let _ = events_tx.send(evt);
            }
            ServiceEvent::ServiceRemoved(_ty, fullname) => {
                let instance_name = instance_from_fullname(&fullname);
                let removed = {
                    let mut map = peers.lock().expect("peer map mutex poisoned");
                    map.remove(&instance_name).is_some()
                };
                if removed {
                    let _ = events_tx.send(PeerEvent::Removed { instance_name });
                }
            }
            ServiceEvent::SearchStopped(_) => break,
            other => {
                debug!(?other, "mdns event ignored");
            }
        }
    }
    debug!("mdns event bridge exited");
}

/// Filter out addresses that can't be used for LAN communication.
/// Excludes loopback (127.0.0.0/8, ::1) and link-local (169.254.0.0/16, fe80::/10).
fn is_usable_lan_address(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            // Exclude loopback (127.0.0.0/8) and link-local (169.254.0.0/16)
            !v4.is_loopback() && !v4.is_link_local()
        }
        IpAddr::V6(v6) => {
            // Exclude loopback (::1) and link-local (fe80::/10)
            !v6.is_loopback() && !v6.segments()[0] & 0xffc0 == 0xfe80
        }
    }
}

fn peer_from_service_info(info: &ServiceInfo, own_fp: &str) -> DiscoveredPeer {
    let instance_name = instance_from_fullname(info.get_fullname());
    let addrs: Vec<IpAddr> = info
        .get_addresses()
        .iter()
        .copied()
        .filter(|addr| is_usable_lan_address(addr))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let props = info.get_properties();
    let fingerprint_hex = props
        .get_property_val_str(TXT_FINGERPRINT)
        .unwrap_or("")
        .to_string();
    let host_id_hex = props
        .get_property_val_str(TXT_HOST_ID)
        .unwrap_or("")
        .to_string();
    let data_port = props
        .get_property_val_str(TXT_DATA_PORT)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    let is_self = !own_fp.is_empty() && fingerprint_hex == own_fp;

    DiscoveredPeer {
        instance_name,
        addrs,
        port: info.get_port(),
        data_port,
        fingerprint_hex,
        host_id_hex,
        is_self,
    }
}

/// `My-Mac._mousefly._udp.local.` → `My-Mac`.
fn instance_from_fullname(fullname: &str) -> String {
    let suffix = format!(".{SERVICE_TYPE}");
    if let Some(stripped) = fullname.strip_suffix(&suffix) {
        return stripped.to_string();
    }
    // Fallback: take everything before the first dot.
    fullname
        .split_once('.')
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| fullname.to_string())
}

/// mdns-sd hostnames must be DNS-safe-ish: ASCII letters, digits, `-`. Anything
/// else (spaces, dots, accented chars from "Dorian's MBP") gets replaced with
/// `-` so we don't get a registration error from the responder.
fn sanitize_hostname(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if out.is_empty() {
        out = "mousefly".to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn instance_from_fullname_strips_service() {
        assert_eq!(
            instance_from_fullname("studio._mousefly._udp.local."),
            "studio"
        );
        assert_eq!(instance_from_fullname("studio"), "studio");
    }

    #[test]
    fn sanitize_hostname_replaces_unsafe_chars() {
        assert_eq!(sanitize_hostname("Dorian's MBP"), "Dorian-s-MBP");
        assert_eq!(sanitize_hostname(""), "mousefly");
        assert_eq!(sanitize_hostname("plain-name"), "plain-name");
    }

    /// One advertiser, one browser, on real (non-loopback) interfaces. Skipped
    /// in CI because mDNS over multicast is environment-dependent; run with
    /// `cargo test -p mousefly-discovery -- --ignored` on a machine with a
    /// real interface.
    #[ignore]
    #[tokio::test(flavor = "multi_thread")]
    async fn advertise_and_browse_roundtrip() {
        let cfg = AdvertiseConfig {
            instance_name: "mousefly-test-instance".to_string(),
            port: 65123,
            data_port: 65124,
            fingerprint_hex: "ff".repeat(32),
            host_id_hex: "ee".repeat(32),
        };
        let advertiser = Advertiser::start(cfg.clone()).expect("advertiser starts");
        let browser = Browser::start("0".repeat(64)).expect("browser starts");
        let mut events = browser.events();

        let peer = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                match events.recv().await {
                    Ok(PeerEvent::Added(p)) | Ok(PeerEvent::Updated(p))
                        if p.instance_name == cfg.instance_name =>
                    {
                        return p
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => panic!("event channel closed"),
                }
            }
        })
        .await
        .expect("peer discovered within timeout");

        assert_eq!(peer.port, cfg.port);
        assert_eq!(peer.fingerprint_hex, cfg.fingerprint_hex);
        assert_eq!(peer.host_id_hex, cfg.host_id_hex);
        assert!(!peer.is_self);

        advertiser.stop().expect("advertiser stops");
        browser.stop().expect("browser stops");
    }
}
