//! Linkplay's half-hearted hex encoding.
//!
//! Some firmwares return `Title`/`Artist`/`Album` (and occasionally device
//! names) as hex-encoded UTF-8, others return plain text — sometimes both in
//! the same response. There is no flag to tell them apart, so the decode is a
//! heuristic: try hex → UTF-8, and hand back the original string whenever that
//! fails. Which firmware does what is an open question for the FR-23 spike
//! (docs/firmware-notes.md §6).

use serde::{Deserialize, Deserializer};

/// Placeholders Linkplay uses for "no metadata"; they are not hex and not worth
/// showing to the user.
const PLACEHOLDERS: [&str; 2] = ["un_known", "unknown"];

/// Decode a possibly hex-encoded Linkplay string.
///
/// Falls back to the input verbatim on anything suspicious (odd length, a
/// non-hex digit, bytes that aren't valid UTF-8), so plain-text firmwares pass
/// straight through. The residual false-positive — a title that is legitimately
/// all hex digits *and* decodes to valid UTF-8 — is accepted; it needs a real
/// out-of-band flag to fix, which the API doesn't have.
pub fn decode(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() || PLACEHOLDERS.iter().any(|p| trimmed.eq_ignore_ascii_case(p)) {
        return String::new();
    }
    if trimmed.len() % 2 != 0 {
        return s.to_string();
    }
    match hex::decode(trimmed) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| s.to_string()),
        Err(_) => s.to_string(),
    }
}

/// `#[serde(deserialize_with = "hexstr::de")]` for hex-or-plain string fields.
pub fn de<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    // Tolerant by design (brief §9): a missing/null/numeric value becomes "".
    let value = serde_json::Value::deserialize(d)?;
    Ok(match value {
        serde_json::Value::String(s) => decode(&s),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_realistic_hex_payload() {
        // "Bohemian Rhapsody"
        let raw = "426f68656d69616e2052686170736f6479";
        assert_eq!(decode(raw), "Bohemian Rhapsody");
    }

    #[test]
    fn decodes_accented_utf8() {
        // "Sinéad" — the é is two bytes, which is exactly the case a naive
        // latin-1 decode would mangle.
        let raw = hex::encode("Sinéad");
        assert_eq!(decode(&raw), "Sinéad");
    }

    #[test]
    fn passes_plain_text_through() {
        assert_eq!(decode("Nothing Compares 2 U"), "Nothing Compares 2 U");
        // Odd length: cannot be hex.
        assert_eq!(decode("abc"), "abc");
        // Even length but not hex digits.
        assert_eq!(decode("Radiohead"), "Radiohead");
    }

    #[test]
    fn invalid_utf8_falls_back_to_the_raw_string() {
        // 0xff 0xfe is valid hex but not valid UTF-8.
        assert_eq!(decode("fffe"), "fffe");
    }

    #[test]
    fn empty_and_unknown_become_empty() {
        assert_eq!(decode(""), "");
        assert_eq!(decode("   "), "");
        assert_eq!(decode("un_known"), "");
        assert_eq!(decode("UNKNOWN"), "");
    }
}
