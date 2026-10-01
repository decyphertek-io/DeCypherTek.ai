//! The app-lock keystore.
//!
//! Same crypto envelope as the agent's vault (`src/vault.rs` at the repo
//! root): Argon2id(password, salt) -> 32-byte key, AES-256-GCM over the
//! payload, fresh nonce per seal. The lock file holds the app profile —
//! the SSH key that reaches the VM, the pinned host key and a settings
//! snapshot — sealed under the password the user set during install.
//!
//! That single password is the app's whole trust anchor: it unlocks the
//! app (lock screen), and because it decrypts the SSH key, it gates every
//! path into the VM. Wrong password = GCM tag failure = no data.
//!
//! Format: `magic(8) | version(1) | salt(16) | nonce(12) | AES-256-GCM ciphertext`

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{anyhow, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use serde_json::Value;
use std::path::Path;

const MAGIC: &[u8; 8] = b"DCTLOCK1";
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Argon2id, 32 MiB, t=2, p=1 — identical to the agent vault: fast enough
/// for a phone, hard enough offline.
fn argon_params() -> Params {
    Params::new(32 * 1024, 2, 1, Some(KEY_LEN)).expect("valid argon2 params")
}

pub fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_LEN]> {
    if salt.len() != SALT_LEN {
        return Err(anyhow!("bad salt length"));
    }
    let mut key = [0u8; KEY_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params())
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("argon2 failed: {e}"))?;
    Ok(key)
}

fn encrypt(key: &[u8; KEY_LEN], salt: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|e| anyhow!("random nonce: {e}"))?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut header = Vec::with_capacity(8 + 1 + SALT_LEN + NONCE_LEN);
    header.extend_from_slice(MAGIC);
    header.push(VERSION);
    header.extend_from_slice(salt);
    header.extend_from_slice(&nonce);
    let aad = header.clone();
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("encryption failed"))?;
    header.extend_from_slice(&ct);
    Ok(header)
}

fn decrypt(key: &[u8; KEY_LEN], blob: &[u8]) -> Result<Vec<u8>> {
    let header_len = 8 + 1 + SALT_LEN + NONCE_LEN;
    if blob.len() < header_len || &blob[..8] != MAGIC {
        return Err(anyhow!("not a DeCypherTek app lock"));
    }
    if blob[8] != VERSION {
        return Err(anyhow!("app lock version {} unsupported", blob[8]));
    }
    let nonce = &blob[8 + 1 + SALT_LEN..header_len];
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let aad = &blob[..header_len];
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: &blob[header_len..],
                aad,
            },
        )
        .map_err(|_| anyhow!("wrong password or corrupted app lock"))
}

/// Seal the profile under `password`, atomically (tmp + rename).
pub fn setup(password: &str, path: &Path, profile: &Value) -> Result<()> {
    let mut salt = [0u8; SALT_LEN];
    getrandom::getrandom(&mut salt).map_err(|e| anyhow!("random salt: {e}"))?;
    let key = derive_key(password, &salt)?;
    let plaintext = serde_json::to_vec(profile).context("serialize profile")?;
    let blob = encrypt(&key, &salt, &plaintext)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create lock dir")?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &blob).context("write lock tmp")?;
    std::fs::rename(&tmp, path).context("publish lock")?;
    Ok(())
}

/// Does a lock exist (i.e. is this app already provisioned)?
pub fn exists(path: &Path) -> bool {
    path.is_file()
}

/// Decrypt the profile. A wrong password fails the GCM tag check.
pub fn unlock(password: &str, path: &Path) -> Result<Value> {
    let blob = std::fs::read(path).context("read app lock")?;
    let header_len = 8 + 1 + SALT_LEN + NONCE_LEN;
    if blob.len() < header_len || &blob[..8] != MAGIC {
        return Err(anyhow!("not a DeCypherTek app lock"));
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&blob[8 + 1..8 + 1 + SALT_LEN]);
    let key = derive_key(password, &salt)?;
    let plain = decrypt(&key, &blob)?;
    serde_json::from_slice(&plain).context("parse profile")
}

/// Change the app-lock password, keeping the sealed profile as-is.
pub fn rekey(old: &str, new: &str, path: &Path) -> Result<()> {
    let profile = unlock(old, path)?;
    setup(new, path, &profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmpfile(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dct-lock-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("app.lock")
    }

    #[test]
    fn lock_roundtrip_wrong_password_and_rekey() {
        let path = tmpfile("roundtrip");
        let profile = json!({"ssh_user": "decyphertek", "secret": "hunter2"});

        setup("correct horse battery", &path, &profile).unwrap();
        assert!(exists(&path));

        // Wrong password fails cleanly.
        assert!(unlock("wrong", &path).is_err());

        // Right password round-trips.
        let got = unlock("correct horse battery", &path).unwrap();
        assert_eq!(got["secret"], "hunter2");
        assert_eq!(got["ssh_user"], "decyphertek");

        // Rekey keeps the profile but moves the password.
        rekey("correct horse battery", "new passphrase", &path).unwrap();
        assert!(unlock("correct horse battery", &path).is_err());
        let got = unlock("new passphrase", &path).unwrap();
        assert_eq!(got["secret"], "hunter2");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn unlock_rejects_garbage() {
        let path = tmpfile("garbage");
        std::fs::write(&path, b"not a lock file at all").unwrap();
        assert!(unlock("pw", &path).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
