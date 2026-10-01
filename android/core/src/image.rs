//! VM image + firmware downloads — resumable, checksummed, progress-reported.
//!
//! The minimal Mobian qcow2 (~800 MB) and the UEFI firmware blobs land in
//! `<dir>/vm/`. Downloads stream straight to a `.part` file and resume via
//! HTTP Range when the server allows it; the SHA-256 (when provided) is
//! computed while streaming and the file is only published on a match.

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;

/// Progress callback: (bytes_done, total_bytes_or_None, detail).
type OnProgress = Box<dyn Fn(u64, Option<u64>, &str) + Send>;

/// Download `url` into `dest` (via `dest.part`), resuming if possible.
/// `expected_sha256` is verified before the file is published.
fn download(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    on_progress: &OnProgress,
    detail: &str,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).context("create download dir")?;
    }
    let part = dest.with_extension("part");
    let have = part.metadata().map(|m| m.len()).unwrap_or(0);

    let mut req = ureq::get(url);
    if have > 0 {
        req = req.set("Range", &format!("bytes={have}-"));
    }
    let resp = req.call().map_err(|e| anyhow!("download {url}: {e}"))?;

    let (mut reader, total, resume_from) = if resp.status() == 206 && have > 0 {
        let total: u64 = resp
            .header("Content-Range")
            .and_then(|r| r.split('/').next_back())
            .and_then(|t| t.parse().ok())
            .unwrap_or(have);
        (resp.into_reader(), total, have)
    } else {
        let total: Option<u64> = resp.header("Content-Length").and_then(|t| t.parse().ok());
        (resp.into_reader(), total.unwrap_or(0), 0)
    };

    let mut file = if resume_from > 0 {
        std::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .with_context(|| format!("resume {}", part.display()))?
    } else {
        std::fs::File::create(&part).with_context(|| format!("create {}", part.display()))?
    };

    let mut hasher = Sha256::new();
    if resume_from > 0 {
        // Hash what we already have so the final digest covers the file.
        let mut old = std::fs::File::open(&part)?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = old.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
    }

    let mut buf = vec![0u8; 256 * 1024];
    let mut done = resume_from;
    let mut last_report = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| anyhow!("read: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        if done - last_report >= 4 * 1024 * 1024 {
            last_report = done;
            let total_opt = if total > 0 { Some(total) } else { None };
            on_progress(done, total_opt, detail);
        }
    }
    drop(file);

    let digest = format!("{:x}", hasher.finalize());
    if let Some(expected) = expected_sha256 {
        if !expected.eq_ignore_ascii_case(&digest) {
            let _ = std::fs::remove_file(&part);
            bail!(
                "sha256 mismatch for {}: expected {expected}, got {digest}",
                dest.display()
            );
        }
    }

    std::fs::rename(&part, dest).context("publish download")?;
    Ok(())
}

/// Make sure the Mobian image + UEFI firmware exist under `<dir>/vm/`.
/// Returns what is now present.
pub fn ensure(
    dir: &str,
    url: &str,
    sha256: Option<String>,
    bios_url: Option<String>,
    bios_sha256: Option<String>,
    bios_vars_url: Option<String>,
    on_progress: OnProgress,
) -> Result<()> {
    let vm = crate::vm::vm_dir(dir);
    let image = vm.join("mobian.qcow2");
    if !image.is_file() {
        download(url, &image, sha256.as_deref(), &on_progress, "mobian.qcow2")?;
    }
    if let Some(bios) = bios_url {
        let efi = vm.join("QEMU_EFI.fd");
        if !efi.is_file() {
            download(
                &bios,
                &efi,
                bios_sha256.as_deref(),
                &on_progress,
                "QEMU_EFI.fd",
            )?;
        }
    }
    if let Some(vars) = bios_vars_url {
        let tpl = vm.join("vars-template.fd");
        if !tpl.is_file() {
            download(&vars, &tpl, None, &on_progress, "vars-template.fd")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_helper_matches_known_digest() {
        // sha256("hello\n")
        let d = {
            let mut h = Sha256::new();
            h.update(b"hello\n");
            format!("{:x}", h.finalize())
        };
        assert_eq!(
            d,
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
        );
    }
}
