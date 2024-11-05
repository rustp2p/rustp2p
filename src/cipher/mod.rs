#[cfg(feature = "aes-gcm")]
pub mod aes_gcm;

#[cfg(feature = "chacha20-poly1305")]
mod chacha20_poly1305;

#[derive(Clone)]
pub enum Cipher {
    #[cfg(feature = "aes-gcm")]
    AesGcm(Box<aes_gcm::AesGcmCipher>),
    #[cfg(feature = "chacha20-poly1305")]
    ChaCha20Poly1305(chacha20_poly1305::ChaCha20Poly1305Cipher),
    None,
}
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Algorithm {
    #[cfg(feature = "aes-gcm")]
    AesGcm(String),
    #[cfg(feature = "chacha20-poly1305")]
    ChaCha20Poly1305(String),
}
impl From<Algorithm> for Cipher {
    fn from(value: Algorithm) -> Self {
        match value {
            #[cfg(feature = "aes-gcm")]
            Algorithm::AesGcm(p) => Cipher::new_aes_gcm(p),
            #[cfg(feature = "chacha20-poly1305")]
            Algorithm::ChaCha20Poly1305(p) => Cipher::new_chacha20_poly1305(p),
        }
    }
}

#[cfg(feature = "aes-gcm")]
impl Cipher {
    pub fn new_aes_gcm(password: String) -> Self {
        Cipher::AesGcm(Box::new(aes_gcm::cipher(password)))
    }
}
#[cfg(feature = "chacha20-poly1305")]
impl Cipher {
    pub fn new_chacha20_poly1305(password: String) -> Self {
        Cipher::ChaCha20Poly1305(chacha20_poly1305::cipher(password))
    }
}
impl Cipher {
    pub fn decrypt(&self, _extra_info: [u8; 12], payload: &mut [u8]) -> anyhow::Result<usize> {
        match self {
            #[cfg(feature = "aes-gcm")]
            Cipher::AesGcm(c) => c.decrypt(_extra_info, payload),
            #[cfg(feature = "chacha20-poly1305")]
            Cipher::ChaCha20Poly1305(c) => c.decrypt(_extra_info, payload),
            Cipher::None => Ok(payload.len()),
        }
    }
    pub fn encrypt(&self, _extra_info: [u8; 12], _payload: &mut [u8]) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "aes-gcm")]
            Cipher::AesGcm(c) => c.encrypt(_extra_info, _payload),
            #[cfg(feature = "chacha20-poly1305")]
            Cipher::ChaCha20Poly1305(c) => c.encrypt(_extra_info, _payload),
            Cipher::None => Ok(()),
        }
    }
    pub fn reserved_len(&self) -> usize {
        match self {
            #[cfg(feature = "aes-gcm")]
            Cipher::AesGcm(c) => c.reserved_len(),
            #[cfg(feature = "chacha20-poly1305")]
            Cipher::ChaCha20Poly1305(c) => c.reserved_len(),
            Cipher::None => 0,
        }
    }
}
