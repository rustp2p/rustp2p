//! Encryption and decryption support for P2P communication.
//!
//! This module provides cryptographic primitives for securing peer-to-peer communication.
//! It supports multiple encryption algorithms through feature flags.
//!
//! # Supported Algorithms
//!
//! - **AES-GCM**: Advanced Encryption Standard in Galois/Counter Mode
//!   - Requires feature: `aes-gcm-openssl` or `aes-gcm-ring`
//! - **ChaCha20-Poly1305**: ChaCha20 stream cipher with Poly1305 authenticator
//!   - Requires feature: `chacha20-poly1305-openssl` or `chacha20-poly1305-ring`
//!
//! # Examples
//!
//! ```rust,no_run
//! # #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
//! # {
//! use rustp2p::cipher::{Algorithm, Cipher};
//!
//! // Create an encryption algorithm with a password
//! let algo = Algorithm::AesGcm("my-secret-password".to_string());
//!
//! // Convert to a cipher for use
//! let cipher: Cipher = algo.into();
//!
//! // Encrypt data
//! let mut data = b"Hello, World!".to_vec();
//! let nonce = [0u8; 12]; // In practice, use a unique nonce
//! cipher.encrypt(nonce, &mut data).unwrap();
//! # }
//! ```

use std::io;

#[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
pub mod aes_gcm;

#[cfg(any(
    feature = "chacha20-poly1305-openssl",
    feature = "chacha20-poly1305-ring"
))]
pub mod chacha20_poly1305;

/// Active cipher instance for encrypting and decrypting data.
///
/// `Cipher` wraps the actual encryption implementation and provides a uniform
/// interface regardless of which algorithm is selected.
///
/// # Variants
///
/// - `AesGcm` - AES-GCM encryption (requires `aes-gcm-*` feature)
/// - `ChaCha20Poly1305` - ChaCha20-Poly1305 encryption (requires `chacha20-poly1305-*` feature)
/// - `None` - No encryption (plaintext)
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

/// Encryption algorithm specification with password.
///
/// This enum is used to specify which encryption algorithm to use when
/// creating a P2P endpoint. The string parameter is the password/key used
/// for encryption.
///
/// # Examples
///
/// ```rust
/// # #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
/// # {
/// use rustp2p::cipher::Algorithm;
///
/// let algo = Algorithm::AesGcm("my-password".to_string());
/// # }
/// ```
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Algorithm {
    /// AES-GCM encryption with the given password.
    #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
    AesGcm(String),
    /// ChaCha20-Poly1305 encryption with the given password.
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
    /// Creates a new AES-GCM cipher with the given password.
    ///
    /// # Arguments
    ///
    /// * `password` - The password to derive the encryption key from
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(any(feature = "aes-gcm-openssl", feature = "aes-gcm-ring"))]
    /// # {
    /// use rustp2p::cipher::Cipher;
    ///
    /// let cipher = Cipher::new_aes_gcm("my-password".to_string());
    /// # }
    /// ```
    pub fn new_aes_gcm(password: String) -> Self {
        Cipher::AesGcm(Box::new(aes_gcm::cipher(password)))
    }
}
#[cfg(any(
    feature = "chacha20-poly1305-openssl",
    feature = "chacha20-poly1305-ring"
))]
impl Cipher {
    /// Creates a new ChaCha20-Poly1305 cipher with the given password.
    ///
    /// # Arguments
    ///
    /// * `password` - The password to derive the encryption key from
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(any(feature = "chacha20-poly1305-openssl", feature = "chacha20-poly1305-ring"))]
    /// # {
    /// use rustp2p::cipher::Cipher;
    ///
    /// let cipher = Cipher::new_chacha20_poly1305("my-password".to_string());
    /// # }
    /// ```
    pub fn new_chacha20_poly1305(password: String) -> Self {
        Cipher::ChaCha20Poly1305(Box::new(chacha20_poly1305::cipher(password)))
    }
}
impl Cipher {
    /// Decrypts data in place using the cipher.
    ///
    /// # Arguments
    ///
    /// * `_extra_info` - A 12-byte nonce/IV for the decryption
    /// * `payload` - The encrypted data to decrypt (modified in place)
    ///
    /// # Returns
    ///
    /// The length of the decrypted data on success.
    ///
    /// # Errors
    ///
    /// Returns an error if decryption fails (e.g., authentication tag mismatch).
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

    /// Encrypts data in place using the cipher.
    ///
    /// # Arguments
    ///
    /// * `_extra_info` - A 12-byte nonce/IV for the encryption
    /// * `_payload` - The data to encrypt (modified in place)
    ///
    /// # Errors
    ///
    /// Returns an error if encryption fails.
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

    /// Returns the number of extra bytes required for the cipher's authentication tag.
    ///
    /// This is typically 16 bytes for GCM modes. When allocating buffers for
    /// encrypted data, you need to account for this extra space.
    ///
    /// # Returns
    ///
    /// The number of reserved bytes needed (0 if no encryption).
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
