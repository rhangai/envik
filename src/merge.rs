//! Combines every source into the final value of each key.

use std::collections::HashMap;

use crate::env::{EnvFile, EnvKey, EnvValue};

/// Final value per key, plus the order in which keys first appeared. `V` is
/// [`EnvValue`] after merging and `String` once references are resolved.
///
/// # Implementation
///
/// `order` and `values` hold the same keys; only [`Merged::set`] writes
/// either, and [`Merged::iter`] indexes `values` relying on it.
#[derive(Debug)]
pub struct Merged<V> {
    values: HashMap<EnvKey, V>,
    order: Vec<EnvKey>,
}

impl<V> Default for Merged<V> {
    fn default() -> Self {
        Merged {
            values: HashMap::new(),
            order: Vec::new(),
        }
    }
}

impl<V> Merged<V> {
    /// Final value of `key`, if any source set it.
    pub fn get(&self, key: &EnvKey) -> Option<&V> {
        self.values.get(key)
    }

    /// Keys in order of first appearance, each with its final value.
    pub fn iter(&self) -> impl Iterator<Item = (&EnvKey, &V)> {
        self.order.iter().map(|key| (key, &self.values[key]))
    }

    /// Later calls win; the first call fixes the key's position.
    pub fn set(&mut self, key: &EnvKey, value: V) {
        match self.values.get_mut(key) {
            Some(slot) => {
                *slot = value;
            }
            None => {
                self.order.push(key.clone());
                self.values.insert(key.clone(), value);
            }
        }
    }
}

/// Files apply in order, later ones winning; `overrides` (`-e`) apply last,
/// always win, and are taken literally: no `${...}` in them is a reference.
pub fn merge(files: &[EnvFile], overrides: &[(EnvKey, String)]) -> Merged<EnvValue> {
    let mut merged = Merged::default();
    for (key, value) in files.iter().flat_map(EnvFile::entries) {
        merged.set(key, value.clone());
    }
    for (key, text) in overrides {
        merged.set(key, EnvValue::literal(text.clone()));
    }
    merged
}
