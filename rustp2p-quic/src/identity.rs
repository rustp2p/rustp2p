use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io;

/// Stable peer identifier supplied by the application.
///
/// A `PeerId` is the name used by the high-level API for all peer-to-peer
/// operations. It is not derived from the QUIC certificate; applications choose
/// it directly and can decide how to bind it to certificate trust policy.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, Default)]
pub struct PeerId(pub String);

impl PeerId {
    /// Creates a peer id from application-provided text.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns an empty placeholder peer id used only for internal packet validation.
    pub fn unspecified() -> Self {
        Self(String::new())
    }

    /// Returns the broadcast peer id used by discovery control packets.
    pub fn broadcast() -> Self {
        Self("*".to_string())
    }

    /// Returns true when this is the internal empty placeholder id.
    pub fn is_unspecified(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns true when this id targets broadcast control traffic.
    pub fn is_broadcast(&self) -> bool {
        self.0 == "*"
    }

    /// Returns the peer id as application-visible text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for PeerId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for PeerId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for PeerId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<PeerId> for String {
    fn from(value: PeerId) -> Self {
        value.0
    }
}

impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerId({})", self.0)
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Local overlay identity.
///
/// The peer id and QUIC certificate are intentionally decoupled. The `peer_id`
/// is the stable application-level node identifier. The seed deterministically
/// generates the local QUIC certificate key material, but changing the seed does
/// not change the peer id.
#[derive(Clone)]
pub struct Identity {
    peer_id: PeerId,
    seed_hash: [u8; 32],
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
}

impl Identity {
    /// Creates an identity from a peer id and deterministic certificate seed.
    ///
    /// The peer id must be non-empty and must not be the broadcast id. The seed
    /// is hashed and used to generate the self-signed QUIC certificate key.
    pub fn new(peer_id: impl Into<String>, seed: impl AsRef<[u8]>) -> io::Result<Self> {
        let peer_id = PeerId::new(peer_id);
        if peer_id.is_unspecified() || peer_id.is_broadcast() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "peer id must be a non-empty non-broadcast string",
            ));
        }

        let mut seed_hash = [0u8; 32];
        seed_hash.copy_from_slice(&Sha256::digest(seed.as_ref()));
        let key_der = ed25519_pkcs8_from_seed(&seed_hash);
        let key_der_for_rcgen = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der.clone()));
        let key_pair = KeyPair::from_der_and_sign_algo(&key_der_for_rcgen, &rcgen::PKCS_ED25519)
            .map_err(|e| io::Error::other(format!("identity key: {e}")))?;

        let mut params = CertificateParams::new(vec![peer_id.0.clone()])
            .map_err(|e| io::Error::other(format!("certificate params: {e}")))?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| io::Error::other(format!("certificate: {e}")))?;

        Ok(Self {
            peer_id,
            seed_hash,
            cert_der: cert.der().to_vec(),
            key_der,
        })
    }

    /// Returns the application-level peer id for this identity.
    pub fn peer_id(&self) -> PeerId {
        self.peer_id.clone()
    }

    /// Returns the DER-encoded self-signed certificate used by QUIC.
    pub fn certificate_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// Returns the DER-encoded private key used by QUIC.
    pub fn private_key_der(&self) -> &[u8] {
        &self.key_der
    }

    /// Returns the SHA-256 hash of the seed used for deterministic key generation.
    pub fn seed_hash(&self) -> [u8; 32] {
        self.seed_hash
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("peer_id", &self.peer_id)
            .finish_non_exhaustive()
    }
}

fn ed25519_pkcs8_from_seed(seed: &[u8; 32]) -> Vec<u8> {
    let mut der = Vec::with_capacity(48);
    der.extend_from_slice(&[
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ]);
    der.extend_from_slice(seed);
    der
}

#[cfg(test)]
mod tests {
    use super::{Identity, PeerId};

    #[test]
    fn identity_is_deterministic_from_seed() {
        let a = Identity::new("node-a", "seed-a").unwrap();
        let b = Identity::new("node-a", "seed-a").unwrap();
        assert_eq!(a.peer_id(), PeerId::from("node-a"));
        assert_eq!(a.certificate_der(), b.certificate_der());
        assert_eq!(a.private_key_der(), b.private_key_der());
    }

    #[test]
    fn peer_id_is_independent_from_certificate_seed() {
        let a = Identity::new("node-a", "seed-a").unwrap();
        let b = Identity::new("node-a", "seed-b").unwrap();
        assert_eq!(a.peer_id(), b.peer_id());
        assert_ne!(a.certificate_der(), b.certificate_der());
    }

    #[test]
    fn peer_helpers_work() {
        assert!(PeerId::broadcast().is_broadcast());
        assert!(PeerId::unspecified().is_unspecified());
        assert_eq!(PeerId::from("abc").as_str(), "abc");
    }
}
