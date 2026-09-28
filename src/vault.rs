//! The encrypted vault.
//!
//! Everything the agent knows — config, API keys, RAG vector store, Wiki
//! Memory, chat logs — lives under `~/.decyphertek.ai/` as ONE encrypted
//! file: `vault.dct`. On launch the agent asks for the password, decrypts
//! the vault into a private `staging/` directory (0700), runs, and seals
//! everything back into `vault.dct` on the way out.
//!
//! Format: `magic(8) | version(1) | salt(16) | nonce(12) | AES-256-GCM ciphertext`
//! Key: Argon2id(password, salt) -> 32 bytes. Payload: gzip'd tar of staging/.
//! AES-GCM is authenticated: a wrong password fails the tag check.

use crate::paths::Paths;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{anyhow, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

const MAGIC: &[u8; 8] = b"DCTVAULT";
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Argon2id, 32 MiB, t=2, p=1 — fast enough for a phone, hard enough offline.
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
        return Err(anyhow!("not a DeCypherTek vault"));
    }
    if blob[8] != VERSION {
        return Err(anyhow!("vault version {} unsupported", blob[8]));
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
        .map_err(|_| anyhow!("wrong password or corrupted vault"))
}

fn salt_of(blob: &[u8]) -> Option<Vec<u8>> {
    if blob.len() < 8 + 1 + SALT_LEN || &blob[..8] != MAGIC {
        return None;
    }
    Some(blob[8 + 1..8 + 1 + SALT_LEN].to_vec())
}

/// gzip+tar the staging dir and write `vault.dct` atomically.
/// Wipes staging afterwards so the plaintext copy does not linger.
pub fn seal(p: &Paths, key: &[u8; KEY_LEN], salt: &[u8]) -> Result<()> {
    if !p.staging.exists() {
        return Err(anyhow!("nothing to seal: staging dir missing"));
    }
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut b = tar::Builder::new(&mut enc);
        b.append_dir_all(".", &p.staging).context("tar staging")?;
        b.finish().context("finish tar")?;
    }
    let compressed = enc.finish().context("gzip staging")?;
    let blob = encrypt(key, salt, &compressed)?;

    let tmp = p.root.join("vault.dct.tmp");
    std::fs::write(&tmp, &blob).context("write vault tmp")?;
    std::fs::rename(&tmp, &p.vault_file).context("publish vault")?;
    let _ = std::fs::remove_dir_all(&p.staging);
    Ok(())
}

/// Read the vault salt (needed to derive the key before decrypting).
pub fn vault_salt(p: &Paths) -> Result<[u8; SALT_LEN]> {
    let blob = std::fs::read(&p.vault_file).context("read vault")?;
    let salt = salt_of(&blob).ok_or_else(|| anyhow!("not a DeCypherTek vault"))?;
    let mut out = [0u8; SALT_LEN];
    out.copy_from_slice(&salt);
    Ok(out)
}

fn decrypt_to_vec(p: &Paths, key: &[u8; KEY_LEN]) -> Result<Vec<u8>> {
    let blob = std::fs::read(&p.vault_file).context("read vault")?;
    decrypt(key, &blob)
}

/// Verify a password against the sealed vault without extracting anything.
pub fn verify_password(p: &Paths, key: &[u8; KEY_LEN]) -> Result<()> {
    decrypt_to_vec(p, key).map(|_| ())
}

/// Decrypt + extract the vault into staging/.
pub fn unseal(p: &Paths, key: &[u8; KEY_LEN]) -> Result<()> {
    let plain = decrypt_to_vec(p, key)?;
    // Fresh extraction: wipe any stale staging from a previous crash first.
    let _ = std::fs::remove_dir_all(&p.staging);
    std::fs::create_dir_all(&p.staging)?;
    let mut ar = tar::Archive::new(GzDecoder::new(&plain[..]));
    ar.unpack(&p.staging).context("extract vault")?;
    p.ensure_staging()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("dct-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Paths::new(&root)
    }

    #[test]
    fn vault_roundtrip_and_wrong_password() {
        let p = tmpdir("roundtrip");
        p.ensure_staging().unwrap();
        std::fs::write(p.staging.join("memory").join("secret.txt"), "the key is 42").unwrap();

        let salt = [7u8; SALT_LEN];
        let good = derive_key("correct horse battery", &salt).unwrap();
        seal(&p, &good, &salt).unwrap();
        assert!(p.vault_file.exists(), "vault.dct should exist after seal");
        assert!(!p.staging.exists(), "staging must be wiped after seal");

        // Wrong password fails cleanly.
        let bad = derive_key("wrong password entirely", &salt).unwrap();
        assert!(unseal(&p, &bad).is_err());

        // Right password round-trips.
        unseal(&p, &good).unwrap();
        let txt = std::fs::read_to_string(p.staging.join("memory").join("secret.txt")).unwrap();
        assert_eq!(txt, "the key is 42");
        let _ = std::fs::remove_dir_all(&p.root);
    }

    #[test]
    fn key_derivation_is_deterministic() {
        let salt = [1u8; SALT_LEN];
        assert_eq!(
            derive_key("pw", &salt).unwrap(),
            derive_key("pw", &salt).unwrap()
        );
        assert_ne!(
            derive_key("pw", &salt).unwrap(),
            derive_key("PW", &salt).unwrap()
        );
    }
}
