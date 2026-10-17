// artwork — cover lookup
// (FNV-1a-64 hashed local cache under cache/art/).

use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use crate::parv_debug;

const MAX_BYTES: usize = 6 * 1024 * 1024;

static ART_MU: Mutex<()> = Mutex::new(());

/// FNV-1a over the lowercased URL, zero-padded 16 hex chars.
pub fn hash_key(url: &str) -> String {
    let lowered = url.to_lowercase();
    let mut h: u64 = 1469598103934665603;
    for b in lowered.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{h:016x}")
}

pub fn artwork_local_path(url: &str) -> String {
    if url.len() < 8 || !url.starts_with("http") {
        return String::new();
    }
    let _g = ART_MU.lock().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::create_dir_all("cache/art");
    let dest = format!("cache/art/{}.jpg", hash_key(url));
    if std::fs::metadata(&dest).map(|m| m.len() > 0).unwrap_or(false) {
        return dest;
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap_or_default();
    let sent = client.get(url).header("User-Agent", "Mozilla/5.0").send();
    let Ok(resp) = sent else {
        parv_debug!("platforms", "artwork fetch failed url={url}");
        return String::new();
    };
    let status = resp.status().as_u16();
    let mut body = Vec::new();
    let mut reader = resp;
    let mut buf = [0u8; 65536];
    let mut ok = true;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                body.extend_from_slice(&buf[..n]);
                if body.len() > MAX_BYTES {
                    ok = false;
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                ok = false;
                break;
            }
        }
    }
    if !ok || status >= 400 || body.is_empty() {
        parv_debug!("platforms", "artwork fetch failed status={status} url={url}");
        return String::new();
    }
    if std::fs::write(&dest, &body).is_err() {
        return String::new();
    }
    dest
}
