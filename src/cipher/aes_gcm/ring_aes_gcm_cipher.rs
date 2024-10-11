use crate::cipher::aes_gcm::AES_GCM_ENCRYPTION_RESERVED;
use anyhow::anyhow;
use ring::aead;
use ring::aead::{LessSafeKey, UnboundKey};

pub enum AesGcmCipher {
    AesGCM128(LessSafeKey, [u8; 16]),
    AesGCM256(LessSafeKey, [u8; 32]),
}

impl Clone for AesGcmCipher {
    fn clone(&self) -> Self {
        match &self {
            AesGcmCipher::AesGCM128(_, key) => {
                let c =
                    LessSafeKey::new(UnboundKey::new(&aead::AES_128_GCM, key.as_slice()).unwrap());
                AesGcmCipher::AesGCM128(c, *key)
            }
            AesGcmCipher::AesGCM256(_, key) => {
                let c =
                    LessSafeKey::new(UnboundKey::new(&aead::AES_256_GCM, key.as_slice()).unwrap());
                AesGcmCipher::AesGCM256(c, *key)
            }
        }
    }
}

impl AesGcmCipher {
    pub fn new_128(key: [u8; 16]) -> Self {
        let cipher = LessSafeKey::new(UnboundKey::new(&aead::AES_128_GCM, &key).unwrap());
        AesGcmCipher::AesGCM128(cipher, key)
    }
    pub fn new_256(key: [u8; 32]) -> Self {
        let cipher = LessSafeKey::new(UnboundKey::new(&aead::AES_256_GCM, &key).unwrap());
        AesGcmCipher::AesGCM256(cipher, key)
    }
    pub fn decrypt(&self, payload: &mut [u8]) -> anyhow::Result<usize> {
        let data_len = payload.len();
        if data_len < AES_GCM_ENCRYPTION_RESERVED {
            log::error!(
                "Data exception, length too small {}",
                AES_GCM_ENCRYPTION_RESERVED
            );
            return Err(anyhow!("data err"));
        }
        let nonce_raw = payload[data_len - 12..].try_into().unwrap();

        let nonce = aead::Nonce::assume_unique_for_key(nonce_raw);

        let rs = match &self {
            AesGcmCipher::AesGCM128(cipher, _) => {
                cipher.open_in_place(nonce, aead::Aad::empty(), &mut payload[..data_len - 12])
            }
            AesGcmCipher::AesGCM256(cipher, _) => {
                cipher.open_in_place(nonce, aead::Aad::empty(), &mut payload[..data_len - 12])
            }
        };
        if let Err(e) = rs {
            return Err(anyhow!("Decryption failed:{:?}", e));
        }
        return Ok(data_len - AES_GCM_ENCRYPTION_RESERVED);
    }
    /// payload Sufficient length must be reserved
    pub fn encrypt(&self, nonce_raw: [u8; 12], payload: &mut [u8]) -> anyhow::Result<()> {
        let nonce = aead::Nonce::assume_unique_for_key(nonce_raw);
        let data_len = payload.len();
        if data_len < AES_GCM_ENCRYPTION_RESERVED {
            return Err(anyhow!("data length too small"));
        }
        let rs = match &self {
            AesGcmCipher::AesGCM128(cipher, _) => cipher.seal_in_place_separate_tag(
                nonce,
                aead::Aad::empty(),
                &mut payload[..data_len - AES_GCM_ENCRYPTION_RESERVED],
            ),
            AesGcmCipher::AesGCM256(cipher, _) => cipher.seal_in_place_separate_tag(
                nonce,
                aead::Aad::empty(),
                &mut payload[..data_len - AES_GCM_ENCRYPTION_RESERVED],
            ),
        };
        return match rs {
            Ok(tag) => {
                let tag = tag.as_ref();
                if tag.len() != 16 {
                    return Err(anyhow!("Encryption tag length error:{}", tag.len()));
                }
                payload[data_len - AES_GCM_ENCRYPTION_RESERVED
                    ..data_len - AES_GCM_ENCRYPTION_RESERVED + 16]
                    .copy_from_slice(tag);
                payload[data_len - 12..].copy_from_slice(&nonce_raw);
                Ok(())
            }
            Err(e) => Err(anyhow!("Encryption failed:{:?}", e)),
        };
    }
}

#[test]
fn test_aes_gcm() {
    let d = AesGcmCipher::new_256([0; 32]);
    let src = [3; 100];
    let mut data = src;
    d.encrypt([0; 12], &mut data).unwrap();
    println!("{:?}", data);
    let len = d.decrypt(&mut data).unwrap();
    assert_eq!(&data[..len], &src[..len]);
}
