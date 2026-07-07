use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io;

/// Stable 32-byte peer identifier.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, Default)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    pub fn from_public_key(public_key: &[u8]) -> Self {
        let hash = Sha256::digest(public_key);
        let mut out = [0u8; 32];
        out.copy_from_slice(&hash);
        Self(out)
    }

    pub fn unspecified() -> Self {
        Self([0u8; 32])
    }

    pub fn broadcast() -> Self {
        Self([0xff; 32])
    }

    pub fn is_unspecified(&self) -> bool {
        self.0.iter().all(|v| *v == 0)
    }

    pub fn is_broadcast(&self) -> bool {
        self.0.iter().all(|v| *v == 0xff)
    }
}

impl AsRef<[u8]> for PeerId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<[u8; 32]> for PeerId {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl From<PeerId> for [u8; 32] {
    fn from(value: PeerId) -> Self {
        value.0
    }
}

impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerId({})", hex::encode(self.0))
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// 16-byte group code used to isolate P2P overlays.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, Default)]
pub struct GroupCode(pub [u8; 16]);

impl GroupCode {
    pub fn unspecified() -> Self {
        Self([0u8; 16])
    }

    pub fn is_unspecified(&self) -> bool {
        self.0.iter().all(|v| *v == 0)
    }
}

impl AsRef<[u8]> for GroupCode {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<[u8; 16]> for GroupCode {
    fn from(value: [u8; 16]) -> Self {
        Self(value)
    }
}

impl From<u128> for GroupCode {
    fn from(value: u128) -> Self {
        Self(value.to_be_bytes())
    }
}

impl TryFrom<&str> for GroupCode {
    type Error = io::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() > 16 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "group code string exceeds 16 bytes",
            ));
        }
        let mut out = [0u8; 16];
        out[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self(out))
    }
}

impl fmt::Debug for GroupCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GroupCode({})", hex::encode(self.0))
    }
}

/// Local overlay identity used for peer addressing and routing.
#[derive(Clone)]
pub struct Identity {
    private_key: [u8; 32],
    public_key: [u8; 32],
    peer_id: PeerId,
}

impl Identity {
    pub fn generate() -> io::Result<Self> {
        Self::from_bytes(rand::random())
    }

    pub fn from_bytes(private_key: [u8; 32]) -> io::Result<Self> {
        let secret = x25519_dalek::StaticSecret::from(private_key);
        let public = x25519_dalek::PublicKey::from(&secret);
        Self::from_keypair(&private_key, public.as_bytes())
    }

    fn from_keypair(private_key: &[u8], public_key: &[u8]) -> io::Result<Self> {
        if private_key.len() != 32 || public_key.len() != 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "noise identity keys must be 32 bytes",
            ));
        }
        let mut private = [0u8; 32];
        private.copy_from_slice(private_key);
        let mut public = [0u8; 32];
        public.copy_from_slice(public_key);
        Ok(Self {
            private_key: private,
            public_key: public,
            peer_id: PeerId::from_public_key(public_key),
        })
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.private_key
    }

    #[cfg(test)]
    pub(crate) fn private_key(&self) -> &[u8; 32] {
        &self.private_key
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("peer_id", &self.peer_id)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{GroupCode, Identity, PeerId};

    #[test]
    fn identity_round_trips_from_private_key() {
        let id = Identity::generate().unwrap();
        let id2 = Identity::from_bytes(*id.private_key()).unwrap();
        assert_eq!(id.peer_id(), id2.peer_id());
        assert_eq!(id.public_key(), id2.public_key());
    }

    #[test]
    fn peer_and_group_helpers_work() {
        assert!(PeerId::broadcast().is_broadcast());
        assert!(PeerId::unspecified().is_unspecified());
        assert_eq!(
            GroupCode::try_from("abc").unwrap().as_ref()[..3],
            [b'a', b'b', b'c']
        );
    }
}
