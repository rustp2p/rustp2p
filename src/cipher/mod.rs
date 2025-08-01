use std::io;

#[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
pub mod aes_gcm;

#[cfg(any(
    feature = "chacha20-poly1305-openssl",
    feature = "chacha20-poly1305-ring"
))]
pub mod chacha20_poly1305;

#[derive(Clone)]
pub enum Cipher {
    #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
    AesGcm(Box<aes_gcm::AesGcmCipher>),
    #[cfg(any(
        feature = "chacha20-poly1305-openssl",
        feature = "chacha20-poly1305-ring"
    ))]
    ChaCha20Poly1305(Box<chacha20_poly1305::ChaCha20Poly1305Cipher>),
    None,
}
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Algorithm {
    #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
    AesGcm(String),
    #[cfg(any(
        feature = "chacha20-poly1305-openssl",
        feature = "chacha20-poly1305-ring"
    ))]
    ChaCha20Poly1305(String),
}
impl From<Algorithm> for Cipher {
    fn from(value: Algorithm) -> Self {
        match value {
            #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
            Algorithm::AesGcm(p) => Cipher::new_aes_gcm(p),
            #[cfg(any(
                feature = "chacha20-poly1305-openssl",
                feature = "chacha20-poly1305-ring"
            ))]
            Algorithm::ChaCha20Poly1305(p) => Cipher::new_chacha20_poly1305(p),
        }
    }
}

#[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
impl Cipher {
    pub fn new_aes_gcm(password: String) -> Self {
        Cipher::AesGcm(Box::new(aes_gcm::cipher(password)))
    }
}
#[cfg(any(
    feature = "chacha20-poly1305-openssl",
    feature = "chacha20-poly1305-ring"
))]
impl Cipher {
    pub fn new_chacha20_poly1305(password: String) -> Self {
        Cipher::ChaCha20Poly1305(Box::new(chacha20_poly1305::cipher(password)))
    }
}
impl Cipher {
    pub fn decrypt(&self, _extra_info: [u8; 12], payload: &mut [u8]) -> io::Result<usize> {
        match self {
            #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
            Cipher::AesGcm(c) => c.decrypt(_extra_info, payload),
            #[cfg(any(
                feature = "chacha20-poly1305-openssl",
                feature = "chacha20-poly1305-ring"
            ))]
            Cipher::ChaCha20Poly1305(c) => c.decrypt(_extra_info, payload),
            Cipher::None => Ok(payload.len()),
        }
    }
    pub fn encrypt(&self, _extra_info: [u8; 12], _payload: &mut [u8]) -> io::Result<()> {
        match self {
            #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
            Cipher::AesGcm(c) => c.encrypt(_extra_info, _payload),
            #[cfg(any(
                feature = "chacha20-poly1305-openssl",
                feature = "chacha20-poly1305-ring"
            ))]
            Cipher::ChaCha20Poly1305(c) => c.encrypt(_extra_info, _payload),
            Cipher::None => Ok(()),
        }
    }
    pub fn reserved_len(&self) -> usize {
        match self {
            #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
            Cipher::AesGcm(c) => c.reserved_len(),
            #[cfg(any(
                feature = "chacha20-poly1305-openssl",
                feature = "chacha20-poly1305-ring"
            ))]
            Cipher::ChaCha20Poly1305(c) => c.reserved_len(),
            Cipher::None => 0,
        }
    }
}
