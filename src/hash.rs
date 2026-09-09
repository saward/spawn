//! Central place for the content-hashing scheme shared by pinning, lock
//! files, and migration checksums, so every call site hashes and formats
//! consistently.

use twox_hash::xxhash3_128;

/// Hashes `contents` and returns it as a fixed-width lowercase hex string.
pub fn content_hash(contents: &[u8]) -> String {
    format!("{:032x}", xxhash3_128::Hasher::oneshot(contents))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_a_32_character_lowercase_hex_string() {
        let hash = content_hash(b"hello world");
        assert_eq!(hash.len(), 32);
        assert!(hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }

    #[test]
    fn is_deterministic() {
        assert_eq!(content_hash(b"same input"), content_hash(b"same input"));
    }

    #[test]
    fn differs_for_different_input() {
        assert_ne!(content_hash(b"a"), content_hash(b"b"));
    }
}
