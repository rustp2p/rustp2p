pub const ENCRYPTION_RESERVED: usize = 16 + 12;
mod ring_aes_gcm_cipher;

pub use ring_aes_gcm_cipher::*;
use sha2::Digest;

pub fn cipher(password: String) -> AesGcmCipher {
    let key: [u8; 32] = {
        let mut hasher = sha2::Sha256::new();
        hasher.update(password.as_bytes());
        hasher.finalize().into()
    };
    if password.len() > 12 {
        AesGcmCipher::new_256(key)
    } else {
        let mut key_: [u8; 16] = key[16..].try_into().unwrap();
        for (i, byte) in key_.iter_mut().enumerate() {
            *byte ^= key[i];
        }
        AesGcmCipher::new_128(key_)
    }
}
