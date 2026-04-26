//! SPAKE2 pairing handshake + ed25519 long-lived identity + paired-peers
//! store. See PLAN.md §7 for the protocol rationale.

mod handshake;
mod identity;
mod store;

pub use handshake::{
    generate_pairing_code, now_unix, run_initiator, run_responder, PairingError, PairingHandshake,
    PairingResult,
};
pub use identity::{
    default_config_dir, identity_path, load_or_create_identity, paired_peers_path, save_identity,
    Identity,
};
pub use store::{PairedPeer, PairedPeerStore};
