pub use ring_chacha20_poly1305::*;
use sha2::Digest;

pub const ENCRYPTION_RESERVED: usize = 16 + 12;

mod ring_chacha20_poly1305;
pub fn cipher(password: String) -> ChaCha20Poly1305Cipher {
    let mut hasher = sha2::Sha256::new();
    hasher.update(password.as_bytes());
    let key: [u8; 32] = hasher.finalize().into();
    ChaCha20Poly1305Cipher::new_256(key)
}
