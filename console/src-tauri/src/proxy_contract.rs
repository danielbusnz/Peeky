//! Wire constants for the Cloudflare Worker proxy. The console does not depend
//! on the peeky crate, so the headers are duplicated here and `code_format_valid`
//! mirrors CODE_RE from proxy/src/index.ts. If the wire values ever change,
//! update proxy/src/index.ts first, then this module and
//! peeky/src/providers/proxy_contract.rs together.

pub const DEVICE_ID_HEADER: &str = "x-peeky-device-id";
pub const INVITE_CODE_HEADER: &str = "x-peeky-invite-code";

/// Mirrors the proxy's CODE_RE: /^[A-Z0-9][A-Z0-9-]{6,62}[A-Z0-9]$/
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
