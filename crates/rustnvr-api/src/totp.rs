//! Time-based one-time passwords (RFC 6238), the codes of authenticator
//! apps (Google Authenticator, Aegis, 2FAS…): 6 digits, 30-second steps,
//! HMAC-SHA1. Also the recovery codes of the two-step login.

use hmac::{Hmac, KeyInit as _, Mac as _};
use sha1::Sha1;

/// Length of a step, in seconds.
const STEP_SECONDS: i64 = 30;
/// Steps accepted on each side of the current one (clock drift).
const DRIFT_STEPS: i64 = 1;
/// Recovery codes given when the two-step login is turned on.
pub(crate) const RECOVERY_CODES: usize = 10;

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// `bytes` in base32 (RFC 4648), without padding: what the apps read.
pub(crate) fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &b in bytes {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// The bytes of a base32 text; spaces, case and padding do not matter.
pub(crate) fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let value = BASE32
            .iter()
            .position(|&b| char::from(b) == c.to_ascii_uppercase())?;
        buffer = (buffer << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// A new secret: 20 random bytes (160 bits, as RFC 4226 advises), in base32.
pub(crate) fn new_secret() -> Result<String, String> {
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(base32_encode(&bytes))
}

/// The 6-digit code of `secret` for the 30-second `step`.
fn code(secret: &[u8], step: i64) -> u32 {
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(secret) else {
        return u32::MAX;
    };
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let value = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    value % 1_000_000
}

/// The code an authenticator app shows for `secret_base32` at the Unix time
/// `unix_secs` (for tests and tools).
pub fn code_at(secret_base32: &str, unix_secs: i64) -> Option<String> {
    let secret = base32_decode(secret_base32)?;
    Some(format!(
        "{:06}",
        code(&secret, unix_secs.div_euclid(STEP_SECONDS))
    ))
}

/// The step whose code `typed` is, around the time `now_secs` (the current
/// step and its neighbours); `None` when it matches none.
pub(crate) fn matching_step(secret_base32: &str, typed: &str, now_secs: i64) -> Option<i64> {
    let typed: String = typed.chars().filter(|c| !c.is_whitespace()).collect();
    if typed.len() != 6 || !typed.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let typed: u32 = typed.parse().ok()?;
    let secret = base32_decode(secret_base32)?;
    let now = now_secs.div_euclid(STEP_SECONDS);
    (now - DRIFT_STEPS..=now + DRIFT_STEPS).find(|&step| code(&secret, step) == typed)
}

/// Percent-encodes a part of an `otpauth://` address.
fn encode_part(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The `otpauth://` address an app reads from the QR code.
pub(crate) fn otpauth_uri(account: &str, secret_base32: &str) -> String {
    format!(
        "otpauth://totp/RustNVR:{}?secret={secret_base32}&issuer=RustNVR&algorithm=SHA1&digits=6&period=30",
        encode_part(account)
    )
}

/// The QR code of `text`, as an SVG picture.
pub(crate) fn qr_svg(text: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(200, 200)
        .quiet_zone(true)
        .build())
}

/// Letters and digits of the recovery codes, without the ones easily mixed
/// up (0, o, 1, l, i).
const RECOVERY_ALPHABET: &[u8; 31] = b"23456789abcdefghjkmnpqrstuvwxyz";

/// New recovery codes, `xxxx-xxxx` each.
pub(crate) fn new_recovery_codes() -> Result<Vec<String>, String> {
    (0..RECOVERY_CODES)
        .map(|_| {
            let mut bytes = [0u8; 8];
            getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
            let chars: String = bytes
                .iter()
                .map(|b| char::from(RECOVERY_ALPHABET[usize::from(*b) % RECOVERY_ALPHABET.len()]))
                .collect();
            Ok(format!("{}-{}", &chars[..4], &chars[4..]))
        })
        .collect()
}

/// A recovery code as typed, in the form it was given (lower case, without
/// spaces or dash), to compare.
pub(crate) fn normalize_recovery(typed: &str) -> String {
    typed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SHA-1 secret of RFC 6238, appendix B.
    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn gives_the_codes_of_rfc_6238() {
        // The 8-digit codes of the RFC end with these 6 digits.
        assert_eq!(code(RFC_SECRET, 59 / 30), 287_082);
        assert_eq!(code(RFC_SECRET, 1_111_111_109 / 30), 81_804);
        assert_eq!(code(RFC_SECRET, 1_234_567_890 / 30), 5_924);
        assert_eq!(code(RFC_SECRET, 2_000_000_000 / 30), 279_037);
    }

    #[test]
    fn reads_and_writes_base32() {
        let text = base32_encode(RFC_SECRET);
        assert_eq!(text, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(base32_decode(&text).as_deref(), Some(RFC_SECRET));
        assert_eq!(
            base32_decode("gezd gnbv gy3t qojq gezd gnbv gy3t qojq").as_deref(),
            Some(RFC_SECRET)
        );
        assert_eq!(base32_decode("GEZ1"), None);
        let secret = new_secret().expect("secret");
        assert_eq!(
            (secret.len(), base32_decode(&secret).map(|b| b.len())),
            (32, Some(20))
        );
    }

    #[test]
    fn accepts_a_code_around_the_current_time() {
        let secret = base32_encode(RFC_SECRET);
        // 1111111109 is in step 37037036; its neighbours are accepted too.
        assert_eq!(
            matching_step(&secret, "081804", 1_111_111_109),
            Some(37_037_036)
        );
        assert_eq!(
            matching_step(&secret, " 081 804 ", 1_111_111_109 + 30),
            Some(37_037_036)
        );
        assert_eq!(matching_step(&secret, "081804", 1_111_111_109 + 90), None);
        assert_eq!(matching_step(&secret, "81804", 1_111_111_109), None);
        assert_eq!(matching_step(&secret, "08180a", 1_111_111_109), None);
    }

    #[test]
    fn makes_the_address_and_codes_apps_read() {
        let uri = otpauth_uri("anne marie", "ABC");
        assert!(uri.starts_with("otpauth://totp/RustNVR:anne%20marie?secret=ABC&issuer=RustNVR"));
        let svg = qr_svg(&uri).expect("svg");
        assert!(svg.contains("<svg"), "{svg}");
        let codes = new_recovery_codes().expect("codes");
        assert_eq!(codes.len(), RECOVERY_CODES);
        assert!(
            codes
                .iter()
                .all(|c| c.len() == 9 && c.as_bytes()[4] == b'-')
        );
        assert_eq!(normalize_recovery(" AB2C-dEf3 "), "ab2cdef3");
    }
}
