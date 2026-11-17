//! Reads `.env` text into an [`EnvFile`], one line at a time. Every line keeps
//! its original text, so the first input can be reproduced as-is and only the
//! values swapped.
//!
//! # Grammar
//!
//! ```text
//! line      = entry | "#" [" "] entry | text
//! entry     = blanks key blanks "=" blanks value
//! value     = '"' double '"' after_quote
//!           | "'" single "'" after_quote
//!           | unquoted
//! double    = { "${" reference "}" | "\" escape | char }
//! single    = { char }                    ; everything literal
//! unquoted  = { "${" reference "}" | char }
//!                                         ; up to a "#" after a blank, or the
//!                                         ; end; trailing blanks dropped
//! after_quote = [ blank { blank } [ "#" comment ] ]
//! reference = key [ ":-" default ] | "rand(" digits ")"
//! escape    = "n" | '"' | "\" | "$"
//! ```
//!
//! A commented `#KEY=` line is an entry only when the name touches the `#` (or
//! `# `) and the `=`, and the whole entry reads; otherwise it is text.
//!
//! [`Cursor`] walks one line, holding the only position: one method per rule
//! above, each reading from `pos` and moving it forward.

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::env::{
    EnvCall, EnvEntry, EnvEntryState, EnvFile, EnvFileLine, EnvKey, EnvValue, MAX_RAND_LEN,
};

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
pub fn parse_file(src: &str, origin: &Path) -> Result<EnvFile, Vec<ParseError>> {
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
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(EnvFile { lines })
}

/// One line, its line ending included: text, or an active or commented
/// entry.
pub fn parse_line(raw: &str) -> Result<EnvFileLine, ParseErrorKind> {
    let trimmed = raw.trim_start();
    let is_text = trimmed.is_empty() || trimmed.starts_with('#');
    if !is_text {
        let entry = parse_entry(raw, EnvEntryState::Active)?;
        return Ok(EnvFileLine::Entry(entry));
    }
    let Some(entry) = commented_entry(raw) else {
        return Ok(EnvFileLine::Text(raw.to_owned()));
    };
    Ok(EnvFileLine::Entry(entry))
}

/// `#KEY=...` or `# KEY=...`, when it reads as an entry.
fn commented_entry(raw: &str) -> Option<EnvEntry> {
    let after_hash = raw.strip_prefix('#')?;
    let state = EnvEntryState::Commented {
        space_after_hash: after_hash.starts_with(' '),
    };
    // The name must touch the prefix and the `=`: `#  KEY=` and `# KEY =`
    // are prose.
    let (name, _) = raw[state.prefix().len()..].split_once('=')?;
    if !EnvKey::is_valid(name) {
        return None;
    }
    // A value that does not read makes it prose too, not an error.
    let Ok(entry) = parse_entry(raw, state) else {
        return None;
    };
    Some(entry)
}

/// `raw` as an entry, after `state`'s prefix.
fn parse_entry(raw: &str, state: EnvEntryState) -> Result<EnvEntry, ParseErrorKind> {
    let line = raw.trim_end_matches(['\n', '\r']);
    let mut cursor = Cursor {
        line,
        pos: state.prefix().len(),
    };
    let (key, value, span) = cursor.entry()?;
    Ok(EnvEntry {
        key,
        value,
        raw: raw.to_owned(),
        span,
        state,
    })
}

/// Why a value could not be read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ValueError {
    #[error("unclosed quote")]
    UnclosedQuote,
    #[error("unexpected `{0}` after the closing quote")]
    TextAfterQuote(String),
    #[error("unknown escape `\\{0}` (only \\n, \\\", \\\\ and \\$ are allowed)")]
    UnknownEscape(char),
    #[error("unclosed `${{`")]
    UnclosedRef,
    #[error("invalid reference `${{{0}}}`")]
    InvalidRef(String),
    #[error("a default cannot contain another `${{...}}`")]
    NestedRef,
    #[error("unknown function `{0}` (only `rand` exists)")]
    UnknownFunction(String),
    #[error("invalid call `${{{0}}}` (expected `rand(n)` with n from 1 to {MAX_RAND_LEN})")]
    InvalidCall(String),
    #[error("a function call cannot have a default: `${{{0}}}`")]
    DefaultOnCall(String),
}

/// A position in one line, line ending excluded. Each grammar rule is a
/// method that reads from `pos` and leaves it just after what it read.
struct Cursor<'a> {
    line: &'a str,
    /// A byte offset into `line`, so a span is `start..pos`.
    pos: usize,
}

impl Cursor<'_> {
    /// What is left of the line.
    fn rest(&self) -> &str {
        &self.line[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// Consumes `text` if the line continues with it.
    fn eat(&mut self, text: &str) -> bool {
        if !self.rest().starts_with(text) {
            return false;
        }
        self.pos += text.len();
        true
    }

    fn skip_blanks(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    /// `key = value`: the key with the value and where it sits in the line.
    fn entry(&mut self) -> Result<(EnvKey, EnvValue, Range<usize>), ParseErrorKind> {
        let Some(eq) = self.rest().find('=') else {
            return Err(ParseErrorKind::MissingEquals(self.rest().to_owned()));
        };
        let name = self.rest()[..eq].trim();
        let Some(key) = EnvKey::new(name) else {
            return Err(ParseErrorKind::InvalidKey(name.to_owned()));
        };
        self.pos += eq + 1;
        let after_eq = self.pos;
        self.skip_blanks();
        let start = self.pos;
        let value = self.value()?;
        // An empty value sits right after `=`, so a new one written there is
        // not glued to a `#`.
        let span = if self.pos == start {
            after_eq..after_eq
        } else {
            start..self.pos
        };
        Ok((key, value, span))
    }

    /// A value, quoted or not. A quoted one ends at its closing quote.
    fn value(&mut self) -> Result<EnvValue, ValueError> {
        let value = if self.eat("\"") {
            self.double_quoted()?
        } else if self.eat("'") {
            self.single_quoted()?
        } else {
            return self.unquoted();
        };
        self.after_quote()?;
        Ok(value)
    }

    /// `"..."`: escapes and references. The opening `"` is already read.
    fn double_quoted(&mut self) -> Result<EnvValue, ValueError> {
        let mut value = EnvValue::default();
        loop {
            if self.eat("${") {
                self.reference(&mut value, true)?;
                continue;
            }
            match self.bump() {
                None => {
                    return Err(ValueError::UnclosedQuote);
                }
                Some('"') => {
                    return Ok(value);
                }
                Some('\\') => {
                    value.push_char(self.escape()?);
                }
                Some(c) => {
                    value.push_char(c);
                }
            }
        }
    }

    /// `'...'`: everything literal. The opening `'` is already read.
    fn single_quoted(&mut self) -> Result<EnvValue, ValueError> {
        let Some(close) = self.rest().find('\'') else {
            return Err(ValueError::UnclosedQuote);
        };
        let value = EnvValue::literal(self.rest()[..close].to_owned());
        self.pos += close + 1;
        Ok(value)
    }

    /// After a closing quote only blanks may follow, or blanks and a comment.
    fn after_quote(&self) -> Result<(), ValueError> {
        let rest = self.rest();
        let trimmed = rest.trim_start();
        let blank_before = trimmed.len() < rest.len();
        if trimmed.is_empty() || (blank_before && trimmed.starts_with('#')) {
            return Ok(());
        }
        Err(ValueError::TextAfterQuote(trimmed.to_owned()))
    }

    /// Unquoted: literal text and references, no escapes.
    fn unquoted(&mut self) -> Result<EnvValue, ValueError> {
        let end = self.unquoted_end();
        // Only up to `end`: what follows is a comment or trailing blanks.
        let mut text = Cursor {
            line: &self.line[..end],
            pos: self.pos,
        };
        let mut value = EnvValue::default();
        loop {
            if text.eat("${") {
                text.reference(&mut value, false)?;
                continue;
            }
            let Some(c) = text.bump() else {
                break;
            };
            value.push_char(c);
        }
        self.pos = end;
        Ok(value)
    }

    /// Where an unquoted value starting at `pos` ends: before a `#` that
    /// follows a blank, or at the end of the line, trailing blanks dropped.
    fn unquoted_end(&self) -> usize {
        let mut previous_blank = self.line[..self.pos].ends_with(char::is_whitespace);
        let mut end = self.line.len();
        for (index, c) in self.rest().char_indices() {
            if c == '#' && previous_blank {
                end = self.pos + index;
                break;
            }
            previous_blank = c.is_whitespace();
        }
        self.pos + self.line[self.pos..end].trim_end().len()
    }

    /// `\n`, `\"`, `\\` or `\$`. The `\` is already read.
    fn escape(&mut self) -> Result<char, ValueError> {
        match self.bump() {
            Some('n') => Ok('\n'),
            Some(c @ ('"' | '\\' | '$')) => Ok(c),
            Some(c) => Err(ValueError::UnknownEscape(c)),
            None => Err(ValueError::UnclosedQuote),
        }
    }

    /// `${...}`, appended to `value`. The `${` is already read; inside
    /// `"..."` (`quoted`), the default takes escapes.
    fn reference(&mut self, value: &mut EnvValue, quoted: bool) -> Result<(), ValueError> {
        let body = self.reference_body(quoted)?;
        let (name, default) = match body.split_once(":-") {
            Some((name, default)) => (name, Some(default.to_owned())),
            None => (body.as_str(), None),
        };
        if name.contains('(') {
            if default.is_some() {
                return Err(ValueError::DefaultOnCall(body.clone()));
            }
            value.push_call(parse_call(name)?);
            return Ok(());
        }
        let Some(key) = EnvKey::new(name) else {
            return Err(ValueError::InvalidRef(body.clone()));
        };
        value.push_ref(key, default);
        Ok(())
    }

    /// The text up to the closing `}`, which is consumed. Read whole first,
    /// so an error can quote all of it.
    fn reference_body(&mut self, quoted: bool) -> Result<String, ValueError> {
        let mut body = String::new();
        loop {
            if self.rest().starts_with("${") {
                return Err(ValueError::NestedRef);
            }
            match self.bump() {
                None => return Err(ValueError::UnclosedRef),
                Some('}') => return Ok(body),
                Some('"') if quoted => return Err(ValueError::UnclosedRef),
                Some('\\') if quoted => body.push(self.escape()?),
                Some(c) => body.push(c),
            }
        }
    }
}

/// `function(args)`, the text of a `${...}` that holds a call.
fn parse_call(text: &str) -> Result<EnvCall, ValueError> {
    let invalid = || ValueError::InvalidCall(text.to_owned());
    let (function, rest) = text.split_once('(').ok_or_else(invalid)?;
    let args = rest.strip_suffix(')').ok_or_else(invalid)?;
    if function != "rand" {
        return Err(ValueError::UnknownFunction(function.to_owned()));
    }
    let len: usize = args.trim().parse().map_err(|_| invalid())?;
    if !(1..=MAX_RAND_LEN).contains(&len) {
        return Err(invalid());
    }
    Ok(EnvCall::Rand { len })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(text: &str) -> EnvValue {
        EnvValue::literal(text.to_owned())
    }

    fn key(name: &str) -> EnvKey {
        EnvKey::new(name).unwrap()
    }

    /// The entry `parse_line` should give for `raw`.
    fn entry(
        raw: &str,
        name: &str,
        value: EnvValue,
        span: Range<usize>,
        state: EnvEntryState,
    ) -> EnvFileLine {
        EnvFileLine::Entry(EnvEntry {
            key: key(name),
            value,
            raw: raw.to_owned(),
            span,
            state,
        })
    }

    /// The value read from `text`, which stands for what follows `=`.
    fn value(text: &str) -> EnvValue {
        read_value(text).unwrap()
    }

    /// Reads `text` as what follows `=`.
    fn read_value(text: &str) -> Result<EnvValue, ValueError> {
        let mut cursor = Cursor { line: text, pos: 0 };
        cursor.skip_blanks();
        cursor.value()
    }

    const ACTIVE: EnvEntryState = EnvEntryState::Active;

    #[test]
    fn text_lines() {
        for raw in [
            "\n",
            "# blablabla\n",
            "# DEBUG\n",
            "#  DEBUG=1\n",
            "  # DEBUG=1\n",
            "# DEBUG = 1\n",
            "#1A=x\n",
            "# A='unclosed\n",
        ] {
            assert_eq!(
                parse_line(raw).unwrap(),
                EnvFileLine::Text(raw.to_owned()),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn active_entries() {
        assert_eq!(
            parse_line("PORT=3000  # api\n").unwrap(),
            entry("PORT=3000  # api\n", "PORT", lit("3000"), 5..9, ACTIVE),
        );
        assert_eq!(
            parse_line("A='a b'  # c\n").unwrap(),
            entry("A='a b'  # c\n", "A", lit("a b"), 2..7, ACTIVE),
        );
        // A Windows line ending is not part of the value.
        assert_eq!(
            parse_line("J=valor\r\n").unwrap(),
            entry("J=valor\r\n", "J", lit("valor"), 2..7, ACTIVE),
        );
        // An empty value sits right after `=`, not glued to a comment.
        assert_eq!(
            parse_line("H= # x\n").unwrap(),
            entry("H= # x\n", "H", lit(""), 2..2, ACTIVE),
        );
    }

    #[test]
    fn commented_entries() {
        assert_eq!(
            parse_line("# DEBUG=1\n").unwrap(),
            entry(
                "# DEBUG=1\n",
                "DEBUG",
                lit("1"),
                8..9,
                EnvEntryState::Commented {
                    space_after_hash: true
                },
            ),
        );
        assert_eq!(
            parse_line("#DEBUG=1\n").unwrap(),
            entry(
                "#DEBUG=1\n",
                "DEBUG",
                lit("1"),
                7..8,
                EnvEntryState::Commented {
                    space_after_hash: false
                },
            ),
        );
    }

    #[test]
    fn entry_with_reference() {
        let mut abc = EnvValue::default();
        abc.push_ref(key("ABC"), None);
        assert_eq!(
            parse_line("VAR=${ABC}\n").unwrap(),
            entry("VAR=${ABC}\n", "VAR", abc, 4..10, ACTIVE),
        );
    }

    #[test]
    fn line_errors() {
        for (raw, expected) in [
            ("export A=1\n", "invalid variable name `export A`"),
            ("JUSTTEXT\n", "expected KEY=VALUE, found `JUSTTEXT`"),
            // A value error reaches the line.
            ("A=\"open\n", "unclosed quote"),
        ] {
            let error = parse_line(raw).unwrap_err();
            assert_eq!(error.to_string(), expected, "{raw:?}");
        }
    }

    #[test]
    fn references_and_calls() {
        let mut url = EnvValue::default();
        url.push_str("http://");
        url.push_ref(key("HOST"), Some("localhost".to_owned()));
        url.push_str(":");
        url.push_ref(key("PORT"), None);
        assert_eq!(value("http://${HOST:-localhost}:${PORT}"), url);

        let mut secret = EnvValue::default();
        secret.push_call(EnvCall::Rand { len: 32 });
        assert_eq!(value("${rand(32)}"), secret);
    }

    #[test]
    fn unquoted_value_rules() {
        for (text, expected) in [
            ("valor # comentário", "valor"),
            ("a#b", "a#b"),
            (" valor  ", "valor"),
            ("  x y  ", "x y"),
            ("#x", "#x"),
            (" #x", ""),
            ("a\t#b", "a"),
            ("", ""),
            ("a ## b", "a"),
            // `$` without braces is literal.
            ("$VAR", "$VAR"),
            ("a$VAR}b", "a$VAR}b"),
        ] {
            assert_eq!(value(text), lit(expected), "{text:?}");
        }
    }

    #[test]
    fn quoting_forms() {
        for (text, expected) in [
            ("\"a\\nb\"", "a\nb"),
            ("\"x\\\"y\"", "x\"y"),
            ("\"c\\\\d\"", "c\\d"),
            ("\"\\$\"", "$"),
            ("'a b'", "a b"),
            ("'${X}'", "${X}"),
            ("\"a\\${X}b\"", "a${X}b"),
            ("\"$VAR\"", "$VAR"),
        ] {
            assert_eq!(value(text), lit(expected), "{text:?}");
        }
    }

    #[test]
    fn value_errors() {
        for (text, expected) in [
            ("\"open", "unclosed quote"),
            ("'x' y", "unexpected `y` after the closing quote"),
            (
                "\"\\t\"",
                "unknown escape `\\t` (only \\n, \\\", \\\\ and \\$ are allowed)",
            ),
            ("${A", "unclosed `${`"),
            ("${B:-${C}}", "a default cannot contain another `${...}`"),
            ("${nope(1)}", "unknown function `nope` (only `rand` exists)"),
            (
                "${rand(0)}",
                "invalid call `${rand(0)}` (expected `rand(n)` with n from 1 to 1024)",
            ),
            (
                "${rand(8):-x}",
                "a function call cannot have a default: `${rand(8):-x}`",
            ),
        ] {
            let error = read_value(text).unwrap_err();
            assert_eq!(error.to_string(), expected, "{text:?}");
        }
    }
}
