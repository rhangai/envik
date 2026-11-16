//! Reads a `.env` file into lines that keep their original text, so the first
//! input can be reproduced as-is and only the values swapped.

use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// A variable name: `[A-Za-z_][A-Za-z0-9_]*`. Only built through [`Key::new`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key(String);

impl Key {
    /// Returns `None` when `name` is not a valid variable name.
    pub fn new(name: &str) -> Option<Self> {
        let mut chars = name.chars();
        let first = chars.next()?;
        let valid = (first.is_ascii_alphabetic() || first == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        valid.then(|| Key(name.to_owned()))
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

/// One line of the file, with its line ending, exactly as read.
#[derive(Debug)]
pub enum Line {
    /// Blank line or comment: written back untouched.
    Verbatim(String),
    /// `KEY=value`. `span` is where the value sits in `raw`, so it can be
    /// replaced without disturbing spacing or an inline comment.
    Entry {
        key: Key,
        value: String,
        raw: String,
        span: Range<usize>,
    },
}

/// A parsed file, every line kept in order so it can be written back.
#[derive(Debug)]
pub struct EnvFile {
    pub lines: Vec<Line>,
}

impl EnvFile {
    /// The `KEY=value` lines, in file order.
    pub fn entries(&self) -> impl Iterator<Item = (&Key, &str)> {
        self.lines.iter().filter_map(|line| match line {
            Line::Entry { key, value, .. } => Some((key, value.as_str())),
            Line::Verbatim(_) => None,
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
    let content = raw.trim_end_matches(['\n', '\r']);
    let trimmed = content.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(Line::Verbatim(raw.to_owned()));
    }
    let Some(eq) = content.find('=') else {
        return Err(ParseErrorKind::MissingEquals(content.to_owned()));
    };
    let name = content[..eq].trim();
    let key = Key::new(name).ok_or_else(|| ParseErrorKind::InvalidKey(name.to_owned()))?;
    let span = value_span(content, eq + 1);
    let value = content[span.clone()].to_owned();
    Ok(Line::Entry {
        key,
        value,
        raw: raw.to_owned(),
        span,
    })
}

/// Where the value lies in `content`, starting the search at `start` (just
/// after `=`): leading blanks skipped, ends before trailing blanks or a `#`
/// that follows a blank.
/// An empty value sits right after `=`, so a new one is not glued to `#`.
fn value_span(content: &str, start: usize) -> Range<usize> {
    let rest = &content[start..];
    let leading = rest.len() - rest.trim_start().len();
    let body = &rest[leading..];
    let mut end = body.len();
    let mut previous_blank = leading > 0;
    for (index, c) in body.char_indices() {
        if c == '#' && previous_blank {
            end = index;
            break;
        }
        previous_blank = c.is_whitespace();
    }
    let value_len = body[..end].trim_end().len();
    if value_len == 0 {
        return start..start;
    }
    start + leading..start + leading + value_len
}
