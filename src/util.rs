//! Plain helpers shared across modules.

use chrono::Local;

pub fn is_termux() -> bool {
    if std::env::var("TERMUX_VERSION").is_ok() {
        return true;
    }
    if let Ok(prefix) = std::env::var("PREFIX") {
        if prefix.contains("com.termux") {
            return true;
        }
    }
    false
}

pub fn now_iso() -> String {
    Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

pub fn unix_ts() -> i64 {
    Local::now().timestamp()
}

/// URL-encode a query string component (RFC 3986, unreserved chars kept).
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// FNV-1a 32-bit — deterministic across compiler and crate versions,
/// which is what the hashing embedder needs (vectors must stay comparable).
pub fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// FNV-1a 64-bit, used to de-duplicate chunks.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Minimal word-wrap for TUI output.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        if para.trim().is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let sep = if cur.is_empty() { "" } else { " " };
            if cur.chars().count() + sep.len() + word.chars().count() > width && !cur.is_empty() {
                lines.push(cur.clone());
                cur.clear();
                cur.push_str(word);
            } else {
                cur.push_str(sep);
                cur.push_str(word);
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_deterministic() {
        assert_eq!(fnv1a32(b"hello"), fnv1a32(b"hello"));
        assert_ne!(fnv1a32(b"hello"), fnv1a32(b"world"));
        assert_eq!(fnv1a64(b"hello"), fnv1a64(b"hello"));
    }

    #[test]
    fn urlencode_works() {
        assert_eq!(url_encode("a b&c"), "a%20b%26c");
    }

    #[test]
    fn wrapping_respects_width() {
        let out = wrap_text("aaaa bbbb cccc dddd", 9);
        assert!(out.iter().all(|l| l.chars().count() <= 9));
    }
}
