//! The canonical digest value and the domain-separated hash construction every
//! DCG digest is built from.
//!
//! One construction, used everywhere, so that two canonical objects can never
//! collide across type boundaries: every digest is taken under a **frozen
//! domain separator** naming exactly what is being hashed, over
//! **length-framed** fields, so no concatenation of field bytes can be
//! reinterpreted as a different field split.
//!
//! Domain separators are frozen for v1 and are never changed. Changing one
//! would silently change the identity of every historical fact that used it.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// A canonical content digest in its portable wire form, `sha256:<64 hex>`.
///
/// The wire form carries its algorithm so a verifier never has to infer one,
/// and byte comparison of two `Digest` values is exactly digest equality.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Digest(String);

/// The one hash algorithm v1 canonical digests use.
pub const DIGEST_ALGORITHM: &str = "sha256";

impl Digest {
    /// Parse a `sha256:<64 hex>` wire form.
    pub fn parse(value: impl Into<String>) -> crate::FormatResult<Self> {
        let value = value.into();
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err(crate::FormatError::Identity(format!(
                "digest '{value}' must carry its algorithm as 'sha256:<hex>'"
            )));
        };
        if hex.len() != 64 {
            return Err(crate::FormatError::Identity(format!(
                "digest '{value}' must have 64 hex characters, found {}",
                hex.len()
            )));
        }
        if let Some(character) = hex
            .chars()
            .find(|c| !(c.is_ascii_digit() || matches!(c, 'a'..='f')))
        {
            return Err(crate::FormatError::Identity(format!(
                "digest '{value}' contains '{character}'; it must be lowercase hex"
            )));
        }
        Ok(Self(value))
    }

    /// The digest of `bytes`, with no domain separation.
    ///
    /// Only for hashing an opaque byte stream that is already unambiguous —
    /// a resource's content, say. Canonical *structures* always go through
    /// [`domain_hash`] instead, so their digests can never be confused with
    /// one another.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(format!("sha256:{}", hex(&Sha256::digest(bytes))))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for Digest {
    type Error = crate::FormatError;

    fn try_from(value: String) -> crate::FormatResult<Self> {
        Self::parse(value)
    }
}

impl From<Digest> for String {
    fn from(value: Digest) -> String {
        value.0
    }
}

/// Hash length-framed `fields` under the frozen separator `domain`.
///
/// The framing is: the domain length and bytes, the field count, then each
/// field's length and bytes — all lengths big-endian `u64`. Because every
/// field is length-prefixed, no two distinct field lists can produce the same
/// framed input, so a digest can never be forged by moving bytes across a
/// field boundary.
pub fn domain_hash<'a>(domain: &str, fields: impl IntoIterator<Item = &'a [u8]>) -> Digest {
    let fields: Vec<&[u8]> = fields.into_iter().collect();
    let mut framed = Vec::new();
    framed.extend_from_slice(&(domain.len() as u64).to_be_bytes());
    framed.extend_from_slice(domain.as_bytes());
    framed.extend_from_slice(&(fields.len() as u64).to_be_bytes());
    for field in fields {
        framed.extend_from_slice(&(field.len() as u64).to_be_bytes());
        framed.extend_from_slice(field);
    }
    Digest::of_bytes(&framed)
}

/// Canonicalize `value` and hash it under `domain`.
///
/// This is how every canonical *struct* in this crate obtains its digest: the
/// struct is serialized to canonical JSON, and those exact bytes are the single
/// framed field.
pub fn canonical_digest<T: Serialize>(domain: &str, value: &T) -> crate::FormatResult<Digest> {
    let bytes = crate::canonical::canonical_bytes(value)?;
    Ok(domain_hash(domain, [bytes.as_slice()]))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_names_its_algorithm_and_round_trips() {
        let digest = Digest::of_bytes(b"draft");
        assert!(digest.as_str().starts_with("sha256:"));
        assert_eq!(digest.as_str().len(), 7 + 64);
        let encoded = serde_json::to_string(&digest).unwrap();
        assert_eq!(serde_json::from_str::<Digest>(&encoded).unwrap(), digest);
    }

    #[test]
    fn a_malformed_digest_is_refused_rather_than_carried() {
        assert!(Digest::parse("deadbeef").is_err());
        assert!(Digest::parse("sha256:short").is_err());
        assert!(Digest::parse(format!("sha256:{}", "A".repeat(64))).is_err());
        assert!(Digest::parse(format!("sha256:{}", "z".repeat(64))).is_err());
    }

    #[test]
    fn framing_stops_a_field_boundary_from_being_moved() {
        // Without length framing these two would hash identically.
        let split = domain_hash("draft.test/v1", [b"ab".as_slice(), b"c".as_slice()]);
        let joined = domain_hash("draft.test/v1", [b"a".as_slice(), b"bc".as_slice()]);
        assert_ne!(split, joined);
    }

    #[test]
    fn the_domain_separator_partitions_the_digest_space() {
        let left = domain_hash("draft.dcg.observation/v1", [b"x".as_slice()]);
        let right = domain_hash("draft.dcg.observation-run/v1", [b"x".as_slice()]);
        assert_ne!(left, right);
    }

    #[test]
    fn a_canonical_digest_ignores_field_declaration_order() {
        #[derive(serde::Serialize)]
        struct Forward {
            a: u8,
            b: u8,
        }
        #[derive(serde::Serialize)]
        struct Reversed {
            b: u8,
            a: u8,
        }
        assert_eq!(
            canonical_digest("draft.test/v1", &Forward { a: 1, b: 2 }).unwrap(),
            canonical_digest("draft.test/v1", &Reversed { b: 2, a: 1 }).unwrap()
        );
    }
}
