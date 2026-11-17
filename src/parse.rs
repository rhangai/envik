//! Reads `.env` text into an [`EnvFile`]: every line kept with its original
//! text, so the first input can be reproduced as-is and only the values
//! swapped; each value read into an [`EnvValue`], quotes and escapes resolved
//! and `${...}` kept apart.

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
    EnvValue(#[from] ValueError),
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

/// `raw` as a `KEY=value` line, after `state`'s prefix.
fn parse_entry(raw: &str, state: EnvEntryState) -> Result<EnvEntry, ParseErrorKind> {
    let start = state.prefix().len();
    let content = raw[start..].trim_end_matches(['\n', '\r']);
    let Some(eq) = content.find('=') else {
        return Err(ParseErrorKind::MissingEquals(content.to_owned()));
    };
    let name = content[..eq].trim();
    let Some(key) = EnvKey::new(name) else {
        return Err(ParseErrorKind::InvalidKey(name.to_owned()));
    };
    let (value, span) = read_value(&content[eq + 1..])?;
    let value_start = start + eq + 1;
    Ok(EnvEntry {
        key,
        value,
        raw: raw.to_owned(),
        span: value_start + span.start..value_start + span.end,
        state,
    })
}

/// A comment that is exactly `#KEY=...` or `# KEY=...` with a valid value;
/// anything looser (indented, two spaces, no `=`) is plain text.
fn commented_entry(raw: &str) -> Option<EnvEntry> {
    let after_hash = raw.strip_prefix('#')?;
    let space_after_hash = after_hash.starts_with(' ');
    let state = EnvEntryState::Commented { space_after_hash };
    let rest = &raw[state.prefix().len()..];
    let (name, _) = rest.split_once('=')?;
    // The name must touch `#`/`# ` and `=`: `# KEY = x` is prose.
    if !EnvKey::is_valid(name) {
        return None;
    }
    // A value that does not parse makes it prose too, not an error.
    let Ok(entry) = parse_entry(raw, state) else {
        return None;
    };
    Some(entry)
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

/// Reads what follows `=` on a line: the value, and where it sits in `rest`,
/// quotes included, so a caller can replace it and keep the rest of the line.
fn read_value(rest: &str) -> Result<(EnvValue, Range<usize>), ValueError> {
    let start = rest.len() - rest.trim_start().len();
    let value_text = &rest[start..];
    let (value, len) = if let Some(body) = value_text.strip_prefix('"') {
        read_double_quoted(body)?
    } else if let Some(body) = value_text.strip_prefix('\'') {
        read_single_quoted(body)?
    } else {
        let span = unquoted_span(rest);
        return Ok((read_unquoted(&rest[span.clone()])?, span));
    };
    let end = start + 1 + len;
    check_after_quote(&rest[end..])?;
    Ok((value, start..end))
}

/// Where an unquoted value lies in `rest`: leading blanks skipped, ends before
/// trailing blanks or a `#` that follows a blank. An empty value sits at the
/// very start, so a new one is not glued to a `#`.
fn unquoted_span(rest: &str) -> Range<usize> {
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
        return 0..0;
    }
    leading..leading + value_len
}

/// Unquoted: everything literal except `${...}`; no escapes.
fn read_unquoted(text: &str) -> Result<EnvValue, ValueError> {
    let mut value = EnvValue::default();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        // What follows `c`.
        let after = chars.as_str();
        if c == '$' && after.starts_with('{') {
            chars.next();
            read_ref(&mut chars, false, &mut value)?;
        } else {
            value.push_char(c);
        }
    }
    Ok(value)
}

/// `body` starts just after the opening `'`. Returns the value and the bytes
/// consumed, closing quote included.
fn read_single_quoted(body: &str) -> Result<(EnvValue, usize), ValueError> {
    let close = body.find('\'').ok_or(ValueError::UnclosedQuote)?;
    Ok((EnvValue::literal(body[..close].to_owned()), close + 1))
}

/// `body` starts just after the opening `"`. Returns the value and the bytes
/// consumed, closing quote included.
fn read_double_quoted(body: &str) -> Result<(EnvValue, usize), ValueError> {
    let mut value = EnvValue::default();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        // What follows `c`.
        let after = chars.as_str();
        match c {
            '"' => {
                return Ok((value, body.len() - after.len()));
            }
            '\\' => {
                value.push_char(unescape(chars.next())?);
            }
            '$' if after.starts_with('{') => {
                chars.next();
                read_ref(&mut chars, true, &mut value)?;
            }
            _ => {
                value.push_char(c);
            }
        }
    }
    Err(ValueError::UnclosedQuote)
}

/// The character after a `\` inside `"..."`.
fn unescape(next: Option<char>) -> Result<char, ValueError> {
    match next {
        Some('n') => Ok('\n'),
        Some(c @ ('"' | '\\' | '$')) => Ok(c),
        Some(c) => Err(ValueError::UnknownEscape(c)),
        None => Err(ValueError::UnclosedQuote),
    }
}

/// Reads `name}` or `name:-default}` from `chars`, which sits just after
/// `${`, and appends it to `value`. Inside `"..."` (`quoted`), the default takes escapes and a bare `"`
/// means the reference was never closed.
fn read_ref(
    chars: &mut std::str::Chars<'_>,
    quoted: bool,
    value: &mut EnvValue,
) -> Result<(), ValueError> {
    let mut text = String::new();
    loop {
        let Some(c) = chars.next() else {
            return Err(ValueError::UnclosedRef);
        };
        // What follows `c`.
        let after = chars.as_str();
        match c {
            '}' => break,
            '"' if quoted => return Err(ValueError::UnclosedRef),
            '\\' if quoted => text.push(unescape(chars.next())?),
            '$' if after.starts_with('{') => return Err(ValueError::NestedRef),
            _ => text.push(c),
        }
    }
    let (name, default) = match text.split_once(":-") {
        Some((name, default)) => (name, Some(default.to_owned())),
        None => (text.as_str(), None),
    };
    if name.contains('(') {
        if default.is_some() {
            return Err(ValueError::DefaultOnCall(text.clone()));
        }
        value.push_call(read_call(name)?);
        return Ok(());
    }
    let Some(name) = EnvKey::new(name) else {
        return Err(ValueError::InvalidRef(text.clone()));
    };
    value.push_ref(name, default);
    Ok(())
}

/// `function(args)`, the text between `${` and `}`.
fn read_call(text: &str) -> Result<EnvCall, ValueError> {
    let invalid = || ValueError::InvalidCall(text.to_owned());
    let (function, rest) = text.split_once('(').ok_or_else(invalid)?;
    let args = rest.strip_suffix(')').ok_or_else(invalid)?;
    match function {
        "rand" => {
            let len: usize = args.trim().parse().map_err(|_| invalid())?;
            if (1..=MAX_RAND_LEN).contains(&len) {
                Ok(EnvCall::Rand { len })
            } else {
                Err(invalid())
            }
        }
        _ => Err(ValueError::UnknownFunction(function.to_owned())),
    }
}

/// After a closing quote only blanks may follow, or a blank and a comment.
fn check_after_quote(rest: &str) -> Result<(), ValueError> {
    let trimmed = rest.trim_start();
    let blank_before = trimmed.len() < rest.len();
    if trimmed.is_empty() || (blank_before && trimmed.starts_with('#')) {
        Ok(())
    } else {
        Err(ValueError::TextAfterQuote(trimmed.to_owned()))
    }
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
        read_value(text).unwrap().0
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
