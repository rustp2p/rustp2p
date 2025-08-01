use rand::RngCore;
use ring::aead;
use ring::aead::{LessSafeKey, UnboundKey};
use std::io;

use crate::cipher::chacha20_poly1305::ENCRYPTION_RESERVED;

#[derive(Clone)]
pub struct ChaCha20Poly1305Cipher {
    cipher: LessSafeKey,
}
impl ChaCha20Poly1305Cipher {
    pub fn new_256(key: [u8; 32]) -> Self {
        let cipher = LessSafeKey::new(UnboundKey::new(&aead::CHACHA20_POLY1305, &key).unwrap());
        Self { cipher }
    }
    pub fn reserved_len(&self) -> usize {
        ENCRYPTION_RESERVED
    }
}
impl ChaCha20Poly1305Cipher {
    pub fn decrypt(&self, extra_info: [u8; 12], payload: &mut [u8]) -> io::Result<usize> {
        let data_len = payload.len();
        if data_len < ENCRYPTION_RESERVED {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ChaCha20Poly1305 decryption failed: data length too small",
            ));
        }
        let mut nonce_raw: [u8; 12] = payload[data_len - 12..].try_into().unwrap();
        for (i, b) in nonce_raw.iter_mut().enumerate() {
            *b ^= extra_info[i];
        }
        let nonce = aead::Nonce::assume_unique_for_key(nonce_raw);
        let rs =
            self.cipher
                .open_in_place(nonce, aead::Aad::empty(), &mut payload[..data_len - 12]);
        if let Err(e) = rs {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ChaCha20Poly1305 decryption failed: {e:?}"),
            ));
        }
        Ok(data_len - ENCRYPTION_RESERVED)
    }
    pub fn encrypt(&self, extra_info: [u8; 12], payload: &mut [u8]) -> io::Result<()> {
        let data_len = payload.len();
        if data_len < ENCRYPTION_RESERVED {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ChaCha20Poly1305 encryption failed: data length too small",
            ));
        }
        let mut random = [0u8; 12];
        rand::rng().fill_bytes(&mut random);
        let mut nonce_raw = random;
        for (i, b) in nonce_raw.iter_mut().enumerate() {
            *b ^= extra_info[i];
        }
        let nonce = aead::Nonce::assume_unique_for_key(nonce_raw);
        let rs = self.cipher.seal_in_place_separate_tag(
            nonce,
            aead::Aad::empty(),
            &mut payload[..data_len - ENCRYPTION_RESERVED],
        );
        match rs {
            Ok(tag) => {
                let tag = tag.as_ref();
                if tag.len() != 16 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "ChaCha20Poly1305 encryption failed: tag length error",
                    ));
                }
                payload[data_len - ENCRYPTION_RESERVED..data_len - ENCRYPTION_RESERVED + 16]
                    .copy_from_slice(tag);
                payload[data_len - 12..].copy_from_slice(&random);
                Ok(())
            }
            Err(e) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ChaCha20Poly1305 encryption failed: {e:?}"),
            )),
        }
    }
}

#[test]
fn test_chacha20_poly1305() {
    let d = ChaCha20Poly1305Cipher::new_256([0; 32]);
    let src = [3; 100];
    let mut data = src;
    d.encrypt([0; 12], &mut data).unwrap();
    println!("{:?}", data);
    let len = d.decrypt([0; 12], &mut data).unwrap();
    assert_eq!(&data[..len], &src[..len]);
}
