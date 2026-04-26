//! Pairing handshake. Two roles, initiator and responder, run a SPAKE2
//! key-exchange keyed by the user-typed 6-digit code, confirm they derived
//! the same key with an HMAC tag, then exchange ed25519-signed identity
//! claims (host_id, QUIC cert fingerprint, instance name).
//!
//! Wire framing: `[u32 BE length][bincode payload]`. SPAKE2 messages and the
//! confirm/identity payloads all share this framing.

use std::time::{SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity as SpakeIdentity, Password, Spake2};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::identity::Identity;

const CONFIRM_CONTEXT: &[u8] = b"MouseFly-Pair-v1";
const SIG_CONTEXT: &[u8] = b"MouseFly-Pair-Identity-v1";
const INITIATOR_ID: &[u8] = b"mousefly-initiator";
const RESPONDER_ID: &[u8] = b"mousefly-responder";

const MAX_FRAME_BYTES: u32 = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("code mismatch — peer entered the wrong digits")]
    CodeMismatch,
    #[error("signature mismatch — peer forged its identity")]
    BadSignature,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("framing: {0}")]
    Framing(String),
    #[error("crypto: {0}")]
    Crypto(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingResult {
    pub peer_host_id_hex: String,
    pub peer_cert_fingerprint_hex: String,
    pub instance_name: String,
}

#[derive(Serialize, Deserialize)]
struct IdentityClaim {
    host_id_hex: String,
    cert_fingerprint_hex: String,
    instance_name: String,
}

#[derive(Serialize, Deserialize)]
struct SignedIdentity {
    claim_bytes: Vec<u8>,
    signature: Vec<u8>,
}

pub struct PairingHandshake;

pub fn generate_pairing_code() -> String {
    let n: u32 = OsRng.gen_range(0..1_000_000);
    let s = format!("{n:06}");
    format!("{} {}", &s[..3], &s[3..])
}

pub async fn run_initiator<R, W>(
    stream: (R, W),
    code: &str,
    my_identity: &Identity,
    my_cert_fingerprint_hex: &str,
    my_instance_name: &str,
) -> Result<PairingResult, PairingError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    run_role(
        stream,
        Role::Initiator,
        code,
        my_identity,
        my_cert_fingerprint_hex,
        my_instance_name,
    )
    .await
}

pub async fn run_responder<R, W>(
    stream: (R, W),
    code: &str,
    my_identity: &Identity,
    my_cert_fingerprint_hex: &str,
    my_instance_name: &str,
) -> Result<PairingResult, PairingError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    run_role(
        stream,
        Role::Responder,
        code,
        my_identity,
        my_cert_fingerprint_hex,
        my_instance_name,
    )
    .await
}

#[derive(Copy, Clone)]
enum Role {
    Initiator,
    Responder,
}

async fn run_role<R, W>(
    stream: (R, W),
    role: Role,
    code: &str,
    my_identity: &Identity,
    my_cert_fingerprint_hex: &str,
    my_instance_name: &str,
) -> Result<PairingResult, PairingError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (mut r, mut w) = stream;

    // Strip the optional space the GUI may show ("123 456").
    let raw_code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let password = Password::new(raw_code.as_bytes());
    let id_a = SpakeIdentity::new(INITIATOR_ID);
    let id_b = SpakeIdentity::new(RESPONDER_ID);

    let (state, my_msg) = match role {
        Role::Initiator => Spake2::<Ed25519Group>::start_a(&password, &id_a, &id_b),
        Role::Responder => Spake2::<Ed25519Group>::start_b(&password, &id_a, &id_b),
    };

    write_frame(&mut w, &my_msg).await?;
    let peer_msg = read_frame(&mut r).await?;

    let session_key = state
        .finish(&peer_msg)
        .map_err(|e| PairingError::Crypto(format!("spake2 finish: {e:?}")))?;

    // SPAKE2's session key is fine on its own, but hashing it once gives us a
    // fixed-length key with a clear domain ("MouseFly-Pair-v1") and lets us
    // change the KDF later without touching wire compatibility.
    let mut hasher = Sha256::new();
    hasher.update(b"MouseFly-Pair-v1/K");
    hasher.update(&session_key);
    let k: [u8; 32] = hasher.finalize().into();

    // Confirmation tag — both sides MUST derive the same K. If the user typed
    // the wrong code, SPAKE2 still produces *a* key, just a different one on
    // each side, so the tags won't match.
    let my_tag = confirmation_tag(&k, CONFIRM_CONTEXT);
    write_frame(&mut w, &my_tag).await?;
    let peer_tag = read_frame(&mut r).await?;

    if peer_tag.len() != my_tag.len() || !ct_eq(&peer_tag, &my_tag) {
        return Err(PairingError::CodeMismatch);
    }

    // Identity exchange: each side sends a signed claim. The signature is
    // verified against the host_id (ed25519 pubkey) embedded in the claim,
    // proving the sender holds the private key for that host_id.
    let claim = IdentityClaim {
        host_id_hex: my_identity.host_id_hex(),
        cert_fingerprint_hex: my_cert_fingerprint_hex.to_string(),
        instance_name: my_instance_name.to_string(),
    };
    let claim_bytes = bincode::serialize(&claim)
        .map_err(|e| PairingError::Framing(format!("encode claim: {e}")))?;
    let signed_claim_bytes = sig_signing_input(&claim_bytes);
    let signature: Signature = my_identity.signing_key().sign(&signed_claim_bytes);

    let signed = SignedIdentity {
        claim_bytes: claim_bytes.clone(),
        signature: signature.to_bytes().to_vec(),
    };
    let signed_bytes = bincode::serialize(&signed)
        .map_err(|e| PairingError::Framing(format!("encode signed: {e}")))?;
    write_frame(&mut w, &signed_bytes).await?;
    w.flush().await?;

    let peer_signed_bytes = read_frame(&mut r).await?;
    let peer_signed: SignedIdentity = bincode::deserialize(&peer_signed_bytes)
        .map_err(|e| PairingError::Framing(format!("decode signed: {e}")))?;
    let peer_claim: IdentityClaim = bincode::deserialize(&peer_signed.claim_bytes)
        .map_err(|e| PairingError::Framing(format!("decode claim: {e}")))?;

    let peer_host_id_bytes = hex::decode(&peer_claim.host_id_hex)
        .map_err(|e| PairingError::Framing(format!("host_id hex: {e}")))?;
    if peer_host_id_bytes.len() != 32 {
        return Err(PairingError::Framing("host_id wrong length".into()));
    }
    let mut pk_arr = [0u8; 32];
    pk_arr.copy_from_slice(&peer_host_id_bytes);
    let peer_vk = VerifyingKey::from_bytes(&pk_arr)
        .map_err(|e| PairingError::Crypto(format!("peer pubkey: {e}")))?;

    if peer_signed.signature.len() != ed25519_dalek::SIGNATURE_LENGTH {
        return Err(PairingError::Framing("signature wrong length".into()));
    }
    let mut sig_arr = [0u8; ed25519_dalek::SIGNATURE_LENGTH];
    sig_arr.copy_from_slice(&peer_signed.signature);
    let peer_sig = Signature::from_bytes(&sig_arr);
    let peer_signing_input = sig_signing_input(&peer_signed.claim_bytes);
    peer_vk
        .verify(&peer_signing_input, &peer_sig)
        .map_err(|_| PairingError::BadSignature)?;

    Ok(PairingResult {
        peer_host_id_hex: peer_claim.host_id_hex,
        peer_cert_fingerprint_hex: peer_claim.cert_fingerprint_hex,
        instance_name: peer_claim.instance_name,
    })
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn confirmation_tag(key: &[u8; 32], context: &[u8]) -> Vec<u8> {
    // HMAC-SHA256 via the standard ipad/opad construction. We avoid pulling in
    // the `hmac` crate for one tag.
    const BLOCK: usize = 64;
    let mut k_pad = [0u8; BLOCK];
    k_pad[..32].copy_from_slice(key);

    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k_pad[i];
        opad[i] ^= k_pad[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(context);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().to_vec()
}

fn sig_signing_input(claim_bytes: &[u8]) -> Vec<u8> {
    // Domain-separate the signed payload from anything else this key might
    // ever sign in the protocol.
    let mut v = Vec::with_capacity(SIG_CONTEXT.len() + claim_bytes.len());
    v.extend_from_slice(SIG_CONTEXT);
    v.extend_from_slice(claim_bytes);
    v
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> Result<(), PairingError> {
    if payload.len() as u64 > MAX_FRAME_BYTES as u64 {
        return Err(PairingError::Framing(format!(
            "outbound frame too large: {} bytes",
            payload.len()
        )));
    }
    let len = payload.len() as u32;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(payload).await?;
    Ok(())
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Vec<u8>, PairingError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_BYTES {
        return Err(PairingError::Framing(format!(
            "inbound frame too large: {len} bytes"
        )));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn fingerprint(seed: u8) -> String {
        hex::encode([seed; 32])
    }

    #[tokio::test]
    async fn matching_codes_succeed() {
        let (a_side, b_side) = duplex(8192);
        let (a_r, a_w) = tokio::io::split(a_side);
        let (b_r, b_w) = tokio::io::split(b_side);

        let id_a = Identity::generate();
        let id_b = Identity::generate();

        let id_a_hex = id_a.host_id_hex();
        let id_b_hex = id_b.host_id_hex();
        let fp_a = fingerprint(0xaa);
        let fp_b = fingerprint(0xbb);

        let task_a = {
            let id_a = id_a.clone();
            let fp_a = fp_a.clone();
            tokio::spawn(async move {
                run_initiator((a_r, a_w), "123456", &id_a, &fp_a, "Studio (Mac)").await
            })
        };
        let task_b = {
            let id_b = id_b.clone();
            let fp_b = fp_b.clone();
            tokio::spawn(async move {
                run_responder((b_r, b_w), "123 456", &id_b, &fp_b, "Lap (Win)").await
            })
        };

        let res_a = task_a.await.unwrap().expect("initiator ok");
        let res_b = task_b.await.unwrap().expect("responder ok");

        assert_eq!(res_a.peer_host_id_hex, id_b_hex);
        assert_eq!(res_a.peer_cert_fingerprint_hex, fp_b);
        assert_eq!(res_a.instance_name, "Lap (Win)");

        assert_eq!(res_b.peer_host_id_hex, id_a_hex);
        assert_eq!(res_b.peer_cert_fingerprint_hex, fp_a);
        assert_eq!(res_b.instance_name, "Studio (Mac)");
    }

    #[tokio::test]
    async fn mismatched_codes_fail_with_code_mismatch() {
        let (a_side, b_side) = duplex(8192);
        let (a_r, a_w) = tokio::io::split(a_side);
        let (b_r, b_w) = tokio::io::split(b_side);

        let id_a = Identity::generate();
        let id_b = Identity::generate();
        let fp_a = fingerprint(0x11);
        let fp_b = fingerprint(0x22);

        let task_a =
            tokio::spawn(
                async move { run_initiator((a_r, a_w), "111111", &id_a, &fp_a, "A").await },
            );
        let task_b =
            tokio::spawn(
                async move { run_responder((b_r, b_w), "222222", &id_b, &fp_b, "B").await },
            );

        let res_a = task_a.await.unwrap();
        let res_b = task_b.await.unwrap();

        assert!(matches!(res_a, Err(PairingError::CodeMismatch)));
        assert!(matches!(res_b, Err(PairingError::CodeMismatch)));
    }

    #[test]
    fn pairing_code_format() {
        let c = generate_pairing_code();
        assert_eq!(c.len(), 7);
        assert_eq!(&c[3..4], " ");
        let raw: String = c.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert_eq!(raw.len(), 6);
        assert!(raw.chars().all(|c| c.is_ascii_digit()));
    }
}
