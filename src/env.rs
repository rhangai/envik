//! The model of a `.env` file: names, values and lines. [`parse`](crate::parse)
//! builds it, the [`Generator`](crate::Generator) resolves the values, and
//! [`render`](crate::render) writes the lines back.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

/// A variable name: `[A-Za-z_][A-Za-z0-9_]*`. Only built through [`EnvKey::new`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EnvKey(String);

impl EnvKey {
    /// Returns `None` when `name` is not a valid variable name.
    pub fn new(name: &str) -> Option<Self> {
        if !Self::is_valid(name) {
            return None;
        }
        Some(EnvKey(name.to_owned()))
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

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EnvKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One piece of a [`EnvValue`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvValuePart {
    /// Text taken as-is, escapes already resolved.
    Literal(String),
    /// `${name}`, or `${name:-default}` with `default` as literal text.
    Ref {
        name: EnvKey,
        default: Option<String>,
    },
    /// `$(name args)`, already checked.
    Call(EnvCall),
}

/// A function call, `$(name args)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvCall {
    /// `$(rand len)`: `len` characters, 1 to [`MAX_RAND_LEN`].
    Rand { len: usize },
}

/// Longest `$(rand n)`: a guard against a typo asking for megabytes.
pub const MAX_RAND_LEN: usize = 1024;

/// A sequence of [`EnvValuePart`]s, normalized so that equal content compares equal:
/// no empty literal, and no two literals side by side. The empty value has no
/// parts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvValue(Vec<EnvValuePart>);

impl EnvValue {
    /// A value with no references: `'...'` and `-e` produce these.
    pub fn literal(text: String) -> Self {
        let mut value = EnvValue::default();
        value.push_str(&text);
        value
    }

    /// The pieces in order; none for the empty value.
    pub fn parts(&self) -> &[EnvValuePart] {
        &self.0
    }

    /// The text, when the value holds no reference.
    pub fn as_literal(&self) -> Option<&str> {
        match self.0.as_slice() {
            [] => Some(""),
            [EnvValuePart::Literal(text)] => Some(text),
            _ => None,
        }
    }

    /// Appends literal text, joining the literal before it.
    pub fn push_char(&mut self, c: char) {
        match self.0.last_mut() {
            Some(EnvValuePart::Literal(text)) => text.push(c),
            _ => self.0.push(EnvValuePart::Literal(c.into())),
        }
    }

    /// Appends literal text, joining the literal before it; nothing when
    /// `s` is empty.
    pub fn push_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        match self.0.last_mut() {
            Some(EnvValuePart::Literal(text)) => text.push_str(s),
            _ => self.0.push(EnvValuePart::Literal(s.to_owned())),
        }
    }

    /// Appends `${name}`, or `${name:-default}`.
    pub fn push_ref(&mut self, name: EnvKey, default: Option<String>) {
        self.0.push(EnvValuePart::Ref { name, default });
    }

    /// Appends `$(name args)`.
    pub fn push_call(&mut self, call: EnvCall) {
        self.0.push(EnvValuePart::Call(call));
    }
}

/// A parsed file, every line kept in order so it can be written back.
#[derive(Debug, PartialEq, Eq)]
pub struct EnvFile {
    pub lines: Vec<EnvFileLine>,
}

impl EnvFile {
    /// Whether a commented `# KEY=` line offers `key`, so a value for it
    /// turns that line on.
    pub fn offers(&self, key: &EnvKey) -> bool {
        self.lines.iter().any(|line| match line {
            EnvFileLine::Entry(entry) => entry.key == *key && entry.state != EnvEntryState::Active,
            EnvFileLine::Text(_) => false,
        })
    }

    /// The active `KEY=value` lines, in file order; commented ones are not
    /// values.
    pub fn entries(&self) -> impl Iterator<Item = (&EnvKey, &EnvValue)> {
        self.lines.iter().filter_map(|line| {
            let EnvFileLine::Entry(entry) = line else {
                return None;
            };
            if entry.state != EnvEntryState::Active {
                return None;
            }
            Some((&entry.key, &entry.value))
        })
    }
}

/// One line of the file, with its line ending, exactly as read.
#[derive(Debug, PartialEq, Eq)]
pub enum EnvFileLine {
    /// Blank line or comment: written back untouched.
    Text(String),
    /// An active or commented `KEY=value` line.
    Entry(EnvEntry),
}

/// A `KEY=value` line, active or commented out.
#[derive(Debug, PartialEq, Eq)]
pub struct EnvEntry {
    pub key: EnvKey,
    /// As parsed: quotes and escapes resolved, references not expanded.
    pub value: EnvValue,
    /// The whole line, `#` and line ending included.
    pub raw: String,
    /// Where the value sits in `raw`, quotes included, so it can be replaced
    /// without disturbing spacing or an inline comment.
    pub span: Range<usize>,
    /// Active, or commented and so only offered.
    pub state: EnvEntryState,
}

/// Whether an entry is set, or offered as an optional variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvEntryState {
    /// `KEY=value`: a value, always written.
    Active,
    /// `#KEY=value` or `# KEY=value`. Whether it is turned on, kept or
    /// dropped is decided by [`render`](crate::render::render).
    Commented {
        /// `# KEY=` rather than `#KEY=`.
        space_after_hash: bool,
    },
}

impl EnvEntryState {
    /// What comes before `KEY` on the line: `""`, `"#"` or `"# "`.
    pub fn prefix(self) -> &'static str {
        match self {
            EnvEntryState::Active => "",
            EnvEntryState::Commented {
                space_after_hash: false,
            } => "#",
            EnvEntryState::Commented {
                space_after_hash: true,
            } => "# ",
        }
    }
}

/// A value per key, plus the order in which keys first appeared. `V` is
/// [`EnvValue`] before references are resolved and `String` after.
///
/// # Implementation
///
/// `order` and `values` hold the same keys; only [`EnvMap::set`] writes
/// either, and [`EnvMap::iter`] indexes `values` relying on it.
#[derive(Clone, Debug)]
pub struct EnvMap<V> {
    values: HashMap<EnvKey, V>,
    order: Vec<EnvKey>,
}

impl<V> Default for EnvMap<V> {
    fn default() -> Self {
        EnvMap {
            values: HashMap::new(),
            order: Vec::new(),
        }
    }
}

impl<V> EnvMap<V> {
    /// The value of `key`, if it has one.
    pub fn get(&self, key: &EnvKey) -> Option<&V> {
        self.values.get(key)
    }

    /// Keys in order of first appearance, each with its value.
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
