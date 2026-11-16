//! `${rand(n)}`: random-looking text that is the same on every run with the
//! same seed, so regenerating a `.env` does not rotate its secrets.

use crate::key::Key;

/// The characters `rand` draws from: base64url, so the text needs no quoting
/// and fits in a URL. Exactly 64, so each byte maps with no bias.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Changing it changes every generated value, for every user.
const CONTEXT: &str = "envik rand v1";

/// A seed for `${rand(n)}`. Never empty: [`Seed::new`] refuses that.
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
        (!bytes.is_empty()).then(|| Seed {
            key: blake3::derive_key(CONTEXT, bytes),
        })
    }

    /// `len` characters for the `index`-th call inside `key`'s value. Depends
    /// only on the seed, `key`, `index` and `len`: other lines, and their
    /// order, never change it.
    pub fn rand(&self, key: &Key, index: usize, len: usize) -> String {
        let mut hasher = blake3::Hasher::new_keyed(&self.key);
        // Length-prefixed fields, so no two inputs hash the same bytes.
        for field in [
            key.as_str().as_bytes(),
            &(index as u64).to_le_bytes(),
            b"rand",
            &(len as u64).to_le_bytes(),
        ] {
            hasher.update(&(field.len() as u64).to_le_bytes());
            hasher.update(field);
        }
        let mut bytes = vec![0; len];
        hasher.finalize_xof().fill(&mut bytes);
        bytes
            .iter()
            .map(|byte| char::from(ALPHABET[usize::from(byte & 63)]))
            .collect()
    }
}

/// A fresh seed text from the system's random source: 43 base64url
/// characters, about 258 bits.
pub fn new_seed_text() -> Result<String, getrandom::Error> {
    let mut bytes = [0; 43];
    getrandom::fill(&mut bytes)?;
    Ok(bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte & 63)]))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Interpolates `src` as one file with `seed`; returns `name`'s final text.
    fn value_of(src: &str, seed: &str, name: &str) -> String {
        let file = crate::parse::parse(src, Path::new("test.env")).unwrap();
        let merged = crate::merge::merge(&[file], &[]);
        let seed = Seed::new(seed.as_bytes()).unwrap();
        let resolved = crate::interpolate::interpolate(&merged, Some(&seed)).unwrap();
        resolved.get(&Key::new(name).unwrap()).unwrap().clone()
    }

    #[test]
    fn stable_per_key() {
        let alone = value_of("S=${rand(32)}\n", "abc", "S");

        // Pinned: a change here rotates every user's generated secrets.
        assert_eq!(alone, "s86Qp_FQH_rjGu5Gqkeb3yEdkHoRMa_3");
        assert_eq!(value_of("S=${rand(32)}\n", "abc", "S"), alone);
        assert_ne!(value_of("S=${rand(32)}\n", "abd", "S"), alone);
        // Other `rand` lines before or after `S` do not move it.
        assert_eq!(
            value_of("A=${rand(32)}\nB=${rand(8)}\nS=${rand(32)}\n", "abc", "S"),
            alone
        );
        assert_eq!(
            value_of("S=${rand(32)}\nZ=${rand(32)}\n", "abc", "S"),
            alone
        );
    }
}
