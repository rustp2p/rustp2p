use crate::cipher::aes_gcm::ENCRYPTION_RESERVED;
use openssl::symm::{Cipher, Crypter, Mode};
use rand::RngCore;
use std::io;

#[derive(Clone)]
pub enum AesGcmCipher {
    AesGCM128([u8; 16]),
    AesGCM256([u8; 32]),
}

impl AesGcmCipher {
    pub fn new_128(key: [u8; 16]) -> Self {
        AesGcmCipher::AesGCM128(key)
    }
    pub fn new_256(key: [u8; 32]) -> Self {
        AesGcmCipher::AesGCM256(key)
    }
    pub fn reserved_len(&self) -> usize {
        ENCRYPTION_RESERVED
    }
    pub fn decrypt(&self, extra_info: [u8; 12], payload: &mut [u8]) -> io::Result<usize> {
        let data_len = payload.len();
        if data_len < ENCRYPTION_RESERVED {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "AesGcm decryption failed: data length too small",
            ));
        }
        let mut nonce_raw: [u8; 12] = payload[data_len - 12..].try_into().unwrap();
        for (i, b) in nonce_raw.iter_mut().enumerate() {
            *b ^= extra_info[i];
        }

        let cipher = match self {
            AesGcmCipher::AesGCM128(key) => {
                let key_ref = key.as_ref();
                (Cipher::aes_128_gcm(), key_ref)
            }
            AesGcmCipher::AesGCM256(key) => {
                let key_ref = key.as_ref();
                (Cipher::aes_256_gcm(), key_ref)
            }
        };

        // 提取标签（tag）
        let tag = &payload[data_len - ENCRYPTION_RESERVED..data_len - 12];

        // 创建解密器
        let (cipher_type, key_ref) = cipher;
        let mut decrypter = Crypter::new(cipher_type, Mode::Decrypt, key_ref, Some(&nonce_raw))
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("AesGcm decryption failed: {e}"),
                )
            })?;

        // 设置标签
        decrypter.set_tag(tag).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("AesGcm decryption failed: {e}"),
            )
        })?;

        // 解密数据
        let mut decrypted = vec![0; data_len - ENCRYPTION_RESERVED + cipher_type.block_size()];
        let mut count = decrypter
            .update(&payload[..data_len - ENCRYPTION_RESERVED], &mut decrypted)
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("AesGcm decryption failed: {e}"),
                )
            })?;

        count += decrypter.finalize(&mut decrypted[count..]).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("AesGcm decryption failed: {e}"),
            )
        })?;

        // 复制解密后的数据回原缓冲区
        payload[..count].copy_from_slice(&decrypted[..count]);

        Ok(count)
    }
    /// payload Sufficient length must be reserved
    pub fn encrypt(&self, extra_info: [u8; 12], payload: &mut [u8]) -> io::Result<()> {
        let data_len = payload.len();
        if data_len < ENCRYPTION_RESERVED {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "AesGcm encryption failed: data length too small",
            ));
        }
        let mut random = [0u8; 12];
        rand::rng().fill_bytes(&mut random);
        let mut nonce_raw = random;
        for (i, b) in nonce_raw.iter_mut().enumerate() {
            *b ^= extra_info[i];
        }

        let cipher = match self {
            AesGcmCipher::AesGCM128(key) => {
                let key_ref = key.as_ref();
                (Cipher::aes_128_gcm(), key_ref)
            }
            AesGcmCipher::AesGCM256(key) => {
                let key_ref = key.as_ref();
                (Cipher::aes_256_gcm(), key_ref)
            }
        };

        // 创建加密器
        let (cipher_type, key_ref) = cipher;
        let mut encrypter = Crypter::new(cipher_type, Mode::Encrypt, key_ref, Some(&nonce_raw))
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("AesGcm encryption failed: {e}"),
                )
            })?;

        // 加密数据
        let mut encrypted = vec![0; data_len - ENCRYPTION_RESERVED + cipher_type.block_size()];
        let mut count = encrypter
            .update(&payload[..data_len - ENCRYPTION_RESERVED], &mut encrypted)
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("AesGcm encryption failed: {e}"),
                )
            })?;

        count += encrypter.finalize(&mut encrypted[count..]).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("AesGcm encryption failed: {e}"),
            )
        })?;

        // 获取标签
        let mut tag = vec![0; 16];
        encrypter.get_tag(&mut tag).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("AesGcm encryption failed: {e}"),
            )
        })?;

        // 复制加密后的数据回原缓冲区
        payload[..count].copy_from_slice(&encrypted[..count]);

        // 复制标签和随机数
        payload[data_len - ENCRYPTION_RESERVED..data_len - ENCRYPTION_RESERVED + 16]
            .copy_from_slice(&tag);
        payload[data_len - 12..].copy_from_slice(&random);

        Ok(())
    }
}

#[test]
fn test_aes_gcm() {
    let d = AesGcmCipher::new_256([0; 32]);
    let src = [3; 100];
    let mut data = src;
    d.encrypt([0; 12], &mut data).unwrap();
    println!("{:?}", data);
    let len = d.decrypt([0; 12], &mut data).unwrap();
    assert_eq!(&data[..len], &src[..len]);
}
