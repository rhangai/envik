//! A variable's value: how it is read after `=` ([`read`]), how it is held
//! (literal text and `${...}` references kept apart, so expansion never has to
//! re-read quotes or escapes), and how an expanded value is written back
//! ([`write_literal`]). Reading what was written gives the same text.

use std::ops::Range;

use crate::key::Key;

/// One piece of a [`Value`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// Text taken as-is, escapes already resolved.
    Literal(String),
    /// `${name}`, or `${name:-default}` with `default` as literal text.
    Ref { name: Key, default: Option<String> },
}

/// A sequence of [`Part`]s, normalized so that equal content compares equal:
/// no empty literal, and no two literals side by side. The empty value has no
/// parts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Value(Vec<Part>);

impl Value {
    /// A value with no references: `'...'` and `-e` produce these.
    pub fn literal(text: String) -> Self {
        let mut value = Value::default();
        value.push_str(&text);
        value
    }

    pub fn parts(&self) -> &[Part] {
        &self.0
    }

    /// The text, when the value holds no reference.
    pub fn as_literal(&self) -> Option<&str> {
        match self.0.as_slice() {
            [] => Some(""),
            [Part::Literal(text)] => Some(text),
            _ => None,
        }
    }

    pub fn push_char(&mut self, c: char) {
        match self.0.last_mut() {
            Some(Part::Literal(text)) => text.push(c),
            _ => self.0.push(Part::Literal(c.into())),
        }
    }

    pub fn push_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        match self.0.last_mut() {
            Some(Part::Literal(text)) => text.push_str(s),
            _ => self.0.push(Part::Literal(s.to_owned())),
        }
    }

    pub fn push_ref(&mut self, name: Key, default: Option<String>) {
        self.0.push(Part::Ref { name, default });
    }
}

/// Appends `text` as it goes after `KEY=`, so that it reads back as the same
/// literal: bare when every character is safe; otherwise in single quotes,
/// where nothing is special; and in double quotes with escapes only when it
/// holds a `'` or a newline, which single quotes cannot.
pub fn write_literal(text: &str, out: &mut String) {
    if text.chars().all(is_safe_bare) {
        out.push_str(text);
    } else if !text.contains(['\'', '\n']) {
        out.push('\'');
        out.push_str(text);
        out.push('\'');
    } else {
        out.push('"');
        push_escaped(out, text);
        out.push('"');
    }
}

/// Characters that read back as themselves outside quotes, whatever their
/// neighbours: no blank, `#`, quote, `\` or `$`.
fn is_safe_bare(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_-./:@%+,=".contains(c)
}

fn push_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
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
}

/// Reads what follows `=` on a line: the value, and where it sits in `rest`,
/// quotes included, so a caller can replace it and keep the rest of the line.
pub fn read(rest: &str) -> Result<(Value, Range<usize>), ValueError> {
    let start = rest.len() - rest.trim_start().len();
    match rest[start..].chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let body = &rest[start + 1..];
            let (value, len) = if quote == '"' {
                read_double_quoted(body)?
            } else {
                read_single_quoted(body)?
            };
            let end = start + 1 + len;
            check_after_quote(&rest[end..])?;
            Ok((value, start..end))
        }
        _ => {
            let span = unquoted_span(rest);
            Ok((read_unquoted(&rest[span.clone()])?, span))
        }
    }
}

/// Where an unquoted value lies in `rest`: leading blanks skipped, ends before
/// trailing blanks or a `#` that follows a blank. An empty value sits at the
/// very start, so a new one is not glued to a `#`.
fn unquoted_span(rest: &str) -> Range<usize> {
    let start = 0;
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

/// Unquoted: everything literal except `${...}`; no escapes.
fn read_unquoted(text: &str) -> Result<Value, ValueError> {
    let mut value = Value::default();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '$' && chars.clone().next() == Some('{') {
            chars.next();
            let (name, default) = read_ref(&mut chars, false)?;
            value.push_ref(name, default);
        } else {
            value.push_char(c);
        }
    }
    Ok(value)
}

/// `body` starts just after the opening `'`. Returns the value and the bytes
/// consumed, closing quote included.
fn read_single_quoted(body: &str) -> Result<(Value, usize), ValueError> {
    let close = body.find('\'').ok_or(ValueError::UnclosedQuote)?;
    Ok((Value::literal(body[..close].to_owned()), close + 1))
}

/// `body` starts just after the opening `"`. Returns the value and the bytes
/// consumed, closing quote included.
fn read_double_quoted(body: &str) -> Result<(Value, usize), ValueError> {
    let mut value = Value::default();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Ok((value, body.len() - chars.as_str().len())),
            '\\' => value.push_char(unescape(chars.next())?),
            '$' if chars.clone().next() == Some('{') => {
                chars.next();
                let (name, default) = read_ref(&mut chars, true)?;
                value.push_ref(name, default);
            }
            _ => value.push_char(c),
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
/// `${`. Inside `"..."` (`quoted`), the default takes escapes and a bare `"`
/// means the reference was never closed.
fn read_ref(
    chars: &mut std::str::Chars<'_>,
    quoted: bool,
) -> Result<(Key, Option<String>), ValueError> {
    let mut text = String::new();
    loop {
        match chars.next() {
            None => return Err(ValueError::UnclosedRef),
            Some('}') => break,
            Some('"') if quoted => return Err(ValueError::UnclosedRef),
            Some('\\') if quoted => text.push(unescape(chars.next())?),
            Some('$') if chars.clone().next() == Some('{') => {
                return Err(ValueError::NestedRef);
            }
            Some(c) => text.push(c),
        }
    }
    let (name, default) = match text.split_once(":-") {
        Some((name, default)) => (name, Some(default.to_owned())),
        None => (text.as_str(), None),
    };
    let key = Key::new(name).ok_or_else(|| ValueError::InvalidRef(text.clone()))?;
    Ok((key, default))
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

    /// The value after `=` on each line of `src`, as (name, value).
    fn read_lines(src: &str) -> Vec<(String, Value)> {
        src.lines()
            .map(|line| {
                let (name, rest) = line.split_once('=').unwrap();
                (name.to_owned(), read(rest).unwrap().0)
            })
            .collect()
    }

    fn lit(text: &str) -> Value {
        Value::literal(text.to_owned())
    }

    fn entry(key: &str, text: &str) -> (String, Value) {
        (key.to_owned(), lit(text))
    }

    #[test]
    fn roundtrip_table() {
        // Reading: each quoting form gives the value it stands for.
        assert_eq!(
            read_lines(concat!(
                "A=\"a\\nb\"\n",
                "B=\"x\\\"y\"\n",
                "C=\"c\\\\d\"\n",
                "D=\"\\$\"\n",
                "E='a b'\n",
                "F='${X}'\n",
                "G=\"a\\${X}b\"\n",
            )),
            [
                entry("A", "a\nb"),
                entry("B", "x\"y"),
                entry("C", "c\\d"),
                entry("D", "$"),
                entry("E", "a b"),
                entry("F", "${X}"),
                entry("G", "a${X}b"),
            ],
        );

        // Writing then reading back gives the same value, for every character
        // that needs quoting and for literal text that looks like a reference.
        let values = [
            lit(""),
            lit("plain"),
            lit("a=b:c/d"),
            lit("a b"),
            lit(" lead and trail "),
            lit("a#b"),
            lit("a #b"),
            lit("#"),
            lit("x\"y"),
            lit("it's"),
            lit("c\\d"),
            lit("\\"),
            lit("$"),
            lit("$HOME"),
            lit("${X}"),
            lit("a${X:-d}b"),
            lit("a\nb"),
            lit("\n"),
            lit("tab\there"),
            lit("ação"),
        ];
        let back: Vec<Value> = values
            .iter()
            .map(|value| {
                let mut written = String::new();
                write_literal(value.as_literal().unwrap(), &mut written);
                let (back, span) = read(&written).unwrap();
                assert_eq!(span, 0..written.len(), "{written:?}");
                back
            })
            .collect();
        assert_eq!(back, values);
    }
}
