// Wire constants for the Aegis Cloudflare Worker proxy. The header strings
// are the x-aegis-* literals used throughout proxy/src/index.ts;
// code_format_valid mirrors CODE_RE in that file. Do not change these values;
// users have codes and device IDs in flight.

/// Header carrying the per-install UUID device identifier.
pub const DEVICE_ID_HEADER: &str = "x-aegis-device-id";

/// Header carrying the invite code for demo-tier access.
pub const INVITE_CODE_HEADER: &str = "x-aegis-invite-code";

/// Returns true if `s` is a plausible invite code format.
/// Mirrors the proxy's CODE_RE: /^[A-Z0-9][A-Z0-9-]{6,62}[A-Z0-9]$/
/// The proxy is the source of truth for expiry and device limits.
pub fn code_format_valid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if !(8..=64).contains(&bytes.len()) {
        return false;
    }
    let all_valid = bytes
        .iter()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'-');
    if !all_valid {
        return false;
    }
    bytes.first() != Some(&b'-') && bytes.last() != Some(&b'-')
}
