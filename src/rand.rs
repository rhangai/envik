//! `$(rand n)`: random-looking text that is the same on every run with the
//! same seed, so regenerating a `.env` does not rotate its secrets.

use crate::env::EnvKey;

/// The characters `rand` draws from: base64url, so the text needs no quoting
/// and fits in a URL. Exactly 64, so each byte maps with no bias.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Changing it changes every generated value, for every user.
const CONTEXT: &str = "envik rand v1";

/// A seed for `$(rand n)`. Never empty: [`Seed::new`] refuses that.
#[derive(Clone)]
pub struct Seed {
    /// A blake3 key derived from the seed text, not the text itself.
    key: [u8; 32],
}

impl std::fmt::Debug for Seed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Seed(..)")
    }
}

impl Seed {
    /// `None` when `bytes` is empty.
    pub fn new(bytes: &[u8]) -> Option<Self> {
        // An empty seed would make every secret guessable.
        if bytes.is_empty() {
            return None;
        }
        Some(Seed {
            key: blake3::derive_key(CONTEXT, bytes),
        })
    }

    /// `len` characters for the `index`-th call inside `key`'s value. Depends
    /// only on the seed, `key`, `index` and `len`: other lines, and their
    /// order, never change it.
    pub fn rand(&self, key: &EnvKey, index: usize, len: usize) -> String {
        let mut hasher = blake3::Hasher::new_keyed(&self.key);
        update_field(&mut hasher, key.as_str().as_bytes());
        update_field(&mut hasher, &(index as u64).to_le_bytes());
        update_field(&mut hasher, b"rand");
        update_field(&mut hasher, &(len as u64).to_le_bytes());
        let mut bytes = vec![0; len];
        hasher.finalize_xof().fill(&mut bytes);
        to_alphabet(&bytes)
    }
}

/// Feeds `field` after its length, so ("ab", "c") and ("a", "bc") never hash
/// the same bytes.
fn update_field(hasher: &mut blake3::Hasher, field: &[u8]) {
    hasher.update(&(field.len() as u64).to_le_bytes());
    hasher.update(field);
}

/// One [`ALPHABET`] character per byte.
fn to_alphabet(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte & 63)]))
        .collect()
}

/// A fresh seed text from the system's random source: 43 base64url
/// characters, about 258 bits.
pub fn new_seed_text() -> Result<String, getrandom::Error> {
    let mut bytes = [0; 43];
    getrandom::fill(&mut bytes)?;
    Ok(to_alphabet(&bytes))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Interpolates `src` as one file with `seed`; returns `name`'s final text.
    fn value_of(src: &str, seed: &str, name: &str) -> String {
        let file = crate::parse::parse_file(src, Path::new("test.env")).unwrap();
        let merged = crate::merge::merge(&[file], &[]);
        let seed = Seed::new(seed.as_bytes()).unwrap();
        let resolved = crate::interpolate::interpolate(&merged, Some(&seed)).unwrap();
        resolved.get(&EnvKey::new(name).unwrap()).unwrap().clone()
    }

    #[test]
    fn stable_per_key() {
        let alone = value_of("S=$(rand 32)\n", "abc", "S");

        // Pinned: a change here rotates every user's generated secrets.
        assert_eq!(alone, "s86Qp_FQH_rjGu5Gqkeb3yEdkHoRMa_3");
        assert_eq!(value_of("S=$(rand 32)\n", "abc", "S"), alone);
        assert_ne!(value_of("S=$(rand 32)\n", "abd", "S"), alone);
        // Other `rand` lines before or after `S` do not move it.
        assert_eq!(
            value_of("A=$(rand 32)\nB=$(rand 8)\nS=$(rand 32)\n", "abc", "S"),
            alone
        );
        assert_eq!(value_of("S=$(rand 32)\nZ=$(rand 32)\n", "abc", "S"), alone);
    }
}
