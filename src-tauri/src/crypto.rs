use anyhow::{anyhow, Context, Result};
use argon2::{password_hash::SaltString, Argon2};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use rand::{rngs::OsRng, RngCore};

pub fn generate_salt() -> String {
    SaltString::generate(&mut OsRng).as_str().to_string()
}

pub fn derive_key(master_password: &str, salt: &str) -> Result<Vec<u8>> {
    let salt = SaltString::from_b64(salt).context("主密码盐值格式不正确")?;
    let mut key = vec![0_u8; 32];
    Argon2::default()
        .hash_password_into(
            master_password.as_bytes(),
            salt.as_salt().as_str().as_bytes(),
            &mut key,
        )
        .context("主密码派生失败")?;
    Ok(key)
}

pub fn encrypt_to_base64(key: &[u8], plaintext: &[u8]) -> Result<String> {
    if key.len() != 32 {
        return Err(anyhow!("加密密钥长度必须为 32 字节"));
    }
    let cipher = XChaCha20Poly1305::new_from_slice(key).context("初始化加密器失败")?;
    let mut nonce_bytes = [0_u8; 24];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ciphertext = cipher.encrypt(nonce, plaintext).context("加密失败")?;
    let mut payload = nonce_bytes.to_vec();
    payload.extend_from_slice(&ciphertext);
    Ok(STANDARD.encode(payload))
}

pub fn decrypt_from_base64(key: &[u8], payload_base64: &str) -> Result<Vec<u8>> {
    if key.len() != 32 {
        return Err(anyhow!("解密密钥长度必须为 32 字节"));
    }
    let payload = STANDARD.decode(payload_base64).context("密文解码失败")?;
    if payload.len() < 25 {
        return Err(anyhow!("密文格式不正确"));
    }
    let (nonce_bytes, ciphertext) = payload.split_at(24);
    let cipher = XChaCha20Poly1305::new_from_slice(key).context("初始化解密器失败")?;
    let nonce = XNonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .context("解密失败，可能是主密码错误")?;
    Ok(plaintext)
}
