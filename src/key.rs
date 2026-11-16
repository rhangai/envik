//! Variable names.

use std::fmt;

/// A variable name: `[A-Za-z_][A-Za-z0-9_]*`. Only built through [`Key::new`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key(String);

impl Key {
    /// Returns `None` when `name` is not a valid variable name.
    pub fn new(name: &str) -> Option<Self> {
        if !Self::is_valid(name) {
            return None;
        }
        Some(Key(name.to_owned()))
    }

    /// Whether `name` matches `[A-Za-z_][A-Za-z0-9_]*`.
    pub fn is_valid(name: &str) -> bool {
        let mut chars = name.chars();
        let Some(first) = chars.next() else {
            // An empty name.
            return false;
        };
        // The first character is a letter or `_`: `1PORT` is not a name.
        if !(first.is_ascii_alphabetic() || first == '_') {
            return false;
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
