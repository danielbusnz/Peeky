//! Persistent per-install UUID. Identifies this aegis install to the
//! hosted proxy (aegis-proxy on Cloudflare) so it can meter per-device
//! daily usage without a login flow.
//!
//! On first run, generates a v4 UUID and writes it to:
//!   Linux:   $XDG_CONFIG_HOME/aegis/device_id  (or ~/.config/aegis/device_id)
//!   Windows: %APPDATA%\aegis\device_id
//!   macOS:   ~/Library/Application Support/aegis/device_id
//!
//! On subsequent runs, reads the same file. The UUID is the user's "account"
//! for all intents and purposes. Deleting the file gives them a fresh
//! daily quota, which is fine for v0.1. Abuse protection lives in
//! Cloudflare's WAF and per-IP rate limits, not in this identifier.

use std::fs;
use std::io;
use std::path::PathBuf;

use uuid::Uuid;

/// Returns the device id, creating + persisting one if it doesn't exist yet.
pub fn load_or_create() -> io::Result<String> {
    let path = device_id_path()?;

    if let Ok(existing) = fs::read_to_string(&path) {
        let trimmed = existing.trim();
        // Validate shape so a corrupted file doesn't ship a garbage header
        // to the proxy (which would reject it with a 401 anyway, but we
        // can fail cleaner locally).
        if Uuid::parse_str(trimmed).is_ok() {
            return Ok(trimmed.to_string());
        }
        // Anything that isn't a UUID gets overwritten with a fresh one.
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let new_id = Uuid::new_v4().to_string();
    fs::write(&path, &new_id)?;
    Ok(new_id)
}

fn device_id_path() -> io::Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config dir on this platform"))?;
    Ok(base.join("aegis").join("device_id"))
}
