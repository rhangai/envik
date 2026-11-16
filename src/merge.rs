//! Combines every source into the final value of each key.

use std::collections::HashMap;

use crate::parse::{EnvFile, Key};

/// Final value per key, plus the order in which keys first appeared.
///
/// # Implementation
///
/// `order` and `values` hold the same keys; only [`Merged::set`] writes
/// either, and [`Merged::iter`] indexes `values` relying on it.
#[derive(Debug, Default)]
pub struct Merged {
    values: HashMap<Key, String>,
    order: Vec<Key>,
}

impl Merged {
    /// Final value of `key`, if any source set it.
    pub fn get(&self, key: &Key) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// Keys in order of first appearance, each with its final value.
    pub fn iter(&self) -> impl Iterator<Item = (&Key, &str)> {
        self.order
            .iter()
            .map(|key| (key, self.values[key].as_str()))
    }

    /// Later calls win; the first call fixes the key's position.
    fn set(&mut self, key: &Key, value: &str) {
        match self.values.get_mut(key) {
            Some(slot) => value.clone_into(slot),
            None => {
                self.order.push(key.clone());
                self.values.insert(key.clone(), value.to_owned());
            }
        }
    }
}

/// Files apply in order, later ones winning; `overrides` (`-e`) apply last
/// and always win.
pub fn merge(files: &[EnvFile], overrides: &[(Key, String)]) -> Merged {
    let mut merged = Merged::default();
    for (key, value) in files.iter().flat_map(EnvFile::entries) {
        merged.set(key, value);
    }
    for (key, value) in overrides {
        merged.set(key, value);
    }
    merged
}
