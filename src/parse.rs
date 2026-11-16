//! Reads a `.env` file into lines that keep their original text, so the first
//! input can be reproduced as-is and only the values swapped.

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::key::Key;
use crate::value::{self, Value, ValueError};

/// One line of the file, with its line ending, exactly as read.
#[derive(Debug)]
pub enum Line {
    /// Blank line or comment: written back untouched.
    Verbatim(String),
    Entry(Entry),
    /// `#KEY=value` or `# KEY=value`: an optional variable the template
    /// offers. `entry` is the text after `#` or `# `, parsed as a line of its
    /// own.
    /// Whether it is turned on, kept or dropped is decided by
    /// [`render`](crate::render::render).
    Commented {
        /// `# KEY=` rather than `#KEY=`, to write the line back as it was.
        space_after_hash: bool,
        entry: Entry,
    },
}

/// A `KEY=value` line.
#[derive(Debug)]
pub struct Entry {
    pub key: Key,
    pub value: Value,
    /// The whole line, line ending included.
    pub raw: String,
    /// Where the value sits in `raw`, quotes included, so it can be replaced
    /// without disturbing spacing or an inline comment.
    pub span: Range<usize>,
}

/// A parsed file, every line kept in order so it can be written back.
#[derive(Debug)]
pub struct EnvFile {
    pub lines: Vec<Line>,
}

impl EnvFile {
    /// The active `KEY=value` lines, in file order; commented ones are not
    /// values.
    pub fn entries(&self) -> impl Iterator<Item = (&Key, &Value)> {
        self.lines.iter().filter_map(|line| match line {
            Line::Entry(entry) => Some((&entry.key, &entry.value)),
            Line::Verbatim(_) | Line::Commented { .. } => None,
        })
    }
}

/// A line that is not blank, a comment or `KEY=VALUE`.
#[derive(Debug, thiserror::Error)]
#[error("{}:{line}: {kind}", origin.display())]
pub struct ParseError {
    /// The path the caller passed in [`Source`](crate::Source), as given.
    pub origin: PathBuf,
    /// 1-based.
    pub line: usize,
    /// What was wrong with the line.
    pub kind: ParseErrorKind,
}

/// Why a line was rejected.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ParseErrorKind {
    #[error("expected KEY=VALUE, found `{0}`")]
    MissingEquals(String),
    #[error("invalid variable name `{0}`")]
    InvalidKey(String),
    #[error(transparent)]
    Value(#[from] ValueError),
}

/// Parses `src`; `origin` only labels errors. On failure, returns every
/// invalid line, not just the first, so one run reports them all.
pub fn parse(src: &str, origin: &Path) -> Result<EnvFile, Vec<ParseError>> {
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    for (index, raw) in src.split_inclusive('\n').enumerate() {
        match parse_line(raw) {
            Ok(line) => lines.push(line),
            Err(kind) => errors.push(ParseError {
                origin: origin.to_owned(),
                line: index + 1,
                kind,
            }),
        }
    }
    if errors.is_empty() {
        Ok(EnvFile { lines })
    } else {
        Err(errors)
    }
}

fn parse_line(raw: &str) -> Result<Line, ParseErrorKind> {
    let trimmed = raw.trim_start();
    let is_text = trimmed.is_empty() || trimmed.starts_with('#');
    if !is_text {
        return parse_entry(raw).map(Line::Entry);
    }
    let Some((space_after_hash, entry)) = commented_entry(raw) else {
        return Ok(Line::Verbatim(raw.to_owned()));
    };
    Ok(Line::Commented {
        space_after_hash,
        entry,
    })
}

fn parse_entry(raw: &str) -> Result<Entry, ParseErrorKind> {
    let content = raw.trim_end_matches(['\n', '\r']);
    let Some(eq) = content.find('=') else {
        return Err(ParseErrorKind::MissingEquals(content.to_owned()));
    };
    let name = content[..eq].trim();
    let key = Key::new(name).ok_or_else(|| ParseErrorKind::InvalidKey(name.to_owned()))?;
    let (value, span) = value::read(&content[eq + 1..])?;
    let span = eq + 1 + span.start..eq + 1 + span.end;
    Ok(Entry {
        key,
        value,
        raw: raw.to_owned(),
        span,
    })
}

/// A comment that is exactly `#KEY=...` or `# KEY=...` with a valid value;
/// anything looser (indented, two spaces, no `=`) is plain text.
/// Returns whether a space follows the `#`, and the entry after it.
fn commented_entry(raw: &str) -> Option<(bool, Entry)> {
    let after_hash = raw.strip_prefix('#')?;
    let (space_after_hash, rest) = match after_hash.strip_prefix(' ') {
        Some(rest) => (true, rest),
        None => (false, after_hash),
    };
    let (name, _) = rest.split_once('=')?;
    // The name must touch `#`/`# ` and `=`: `# KEY = x` is prose.
    if !Key::is_valid(name) {
        return None;
    }
    // A value that does not parse makes it prose too, not an error.
    let Ok(entry) = parse_entry(rest) else {
        return None;
    };
    Some((space_after_hash, entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `KEY=value` of `src`, as (name, value).
    fn read(src: &str) -> Vec<(String, Value)> {
        let file = parse(src, Path::new("test.env")).unwrap();
        file.entries()
            .map(|(key, value)| (key.as_str().to_owned(), value.clone()))
            .collect()
    }

    fn lit(text: &str) -> Value {
        Value::literal(text.to_owned())
    }

    fn entry(key: &str, text: &str) -> (String, Value) {
        (key.to_owned(), lit(text))
    }

    #[test]
    fn unquoted_value_rules() {
        assert_eq!(
            read(concat!(
                "A=valor # comentário\n",
                "B=a#b\n",
                "C = valor  \n",
                "  D  =  x y  \n",
                "E=#x\n",
                "F= #x\n",
                "G=a\t#b\n",
                "H=\n",
                "I=a ## b\n",
                "J=valor\r\n",
            )),
            [
                entry("A", "valor"),
                entry("B", "a#b"),
                entry("C", "valor"),
                entry("D", "x y"),
                entry("E", "#x"),
                entry("F", ""),
                entry("G", "a"),
                entry("H", ""),
                entry("I", "a"),
                entry("J", "valor"),
            ],
        );
    }

    #[test]
    fn bare_dollar_is_literal() {
        assert_eq!(
            read("A=$VAR\nB=\"$VAR\"\nC=a$VAR}b\n"),
            [
                entry("A", "$VAR"),
                entry("B", "$VAR"),
                entry("C", "a$VAR}b")
            ],
        );
    }
}
