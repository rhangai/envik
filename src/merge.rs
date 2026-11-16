//! Combines every source into the final value of each key.

use std::collections::HashMap;

use crate::key::Key;
use crate::parse::EnvFile;
use crate::value::Value;

/// Final value per key, plus the order in which keys first appeared.
///
/// # Implementation
///
/// `order` and `values` hold the same keys; only [`Merged::set`] writes
/// either, and [`Merged::iter`] indexes `values` relying on it.
#[derive(Debug, Default)]
pub struct Merged {
    values: HashMap<Key, Value>,
    order: Vec<Key>,
}

impl Merged {
    /// Final value of `key`, if any source set it.
    pub fn get(&self, key: &Key) -> Option<&Value> {
        self.values.get(key)
    }

    /// Keys in order of first appearance, each with its final value.
    pub fn iter(&self) -> impl Iterator<Item = (&Key, &Value)> {
        self.order.iter().map(|key| (key, &self.values[key]))
    }

    /// Later calls win; the first call fixes the key's position.
    fn set(&mut self, key: &Key, value: &Value) {
        match self.values.get_mut(key) {
            Some(slot) => slot.clone_from(value),
            None => {
                self.order.push(key.clone());
                self.values.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Files apply in order, later ones winning; `overrides` (`-e`) apply last,
/// always win, and are taken literally: no `${...}` in them is a reference.
pub fn merge(files: &[EnvFile], overrides: &[(Key, String)]) -> Merged {
    let mut merged = Merged::default();
    for (key, value) in files.iter().flat_map(EnvFile::entries) {
        merged.set(key, value);
    }
    for (key, text) in overrides {
        merged.set(key, &Value::literal(text.clone()));
    }
    merged
}
