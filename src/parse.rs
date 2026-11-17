//! Reads `.env` text into an [`EnvFile`], one line at a time. Every line keeps
//! its original text, so the first input can be reproduced as-is and only the
//! values swapped.
//!
//! # Grammar
//!
//! ```text
//! line      = "#" [" "] key "=" value      ; a commented entry, else text
//!           | blanks [ "#" comment ]        ; text
//!           | blanks key blanks "=" value
//! key       = [A-Za-z_] { [A-Za-z0-9_] }
//! value     = '"' double '"' after_quote
//!           | "'" single "'" after_quote
//!           | unquoted
//! double    = { "$" dollar | "\" escape | char }
//! single    = { char }                    ; everything literal
//! unquoted  = { "$" dollar | char }
//!                                         ; up to a "#" after a blank, or the
//!                                         ; end; trailing blanks dropped. A
//!                                         ; "#" inside "${...}" is text
//! after_quote = [ blank { blank } [ "#" comment ] ]
//! dollar    = "{" reference | "(" call | ""  ; a lone "$" is text
//! reference = key "}" | key ":-" default "}"
//! default   = { char }                    ; no "${" or "$(" inside
//! call      = blanks "rand" blanks digits blanks ")"   ; words, like a shell
//! escape    = "n" | '"' | "\" | "$"
//! ```
//!
//! A commented entry has no blanks around its key, and is text when any of it
//! does not read: `# KEY = x` and `# A='unclosed` are prose, not errors.
//!
//! # Implementation
//!
//! [`Cursor`] walks one line, one character at a time: one method per rule
//! above, each deciding by the next character and consuming what it reads.

use std::iter::Peekable;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::str::CharIndices;

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
    #[error("a default cannot contain `${{...}}` or `$(...)`")]
    NestedRef,
    #[error("unclosed `$(`")]
    UnclosedCall,
    #[error("unknown function `{0}` (only `rand` exists)")]
    UnknownFunction(String),
    #[error("invalid call `$({0})` (expected `$(rand n)` with n from 1 to {MAX_RAND_LEN})")]
    InvalidCall(String),
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
    let Some(entry) = Cursor::new(raw).line()? else {
        return Ok(EnvFileLine::Text(raw.to_owned()));
    };
    Ok(EnvFileLine::Entry(entry))
}

/// A position in one line. Each grammar rule is a method that reads from the
/// next character and leaves the cursor just after what it read. A rule that
/// ends the line (`line`, `entry`, `value`, `unquoted`, `after_quote`) takes
/// the cursor by value, so nothing can read past it.
struct Cursor<'a> {
    /// The line as read, line ending included.
    raw: &'a str,
    /// `raw` without its line ending: what the rules read.
    line: &'a str,
    /// What is left of `line`, each character with its byte offset.
    chars: Peekable<CharIndices<'a>>,
}

impl<'a> Cursor<'a> {
    fn new(raw: &'a str) -> Self {
        let line = raw.trim_end_matches(['\n', '\r']);
        Cursor {
            raw,
            line,
            chars: line.char_indices().peekable(),
        }
    }

    /// The byte offset of the next character, so a span is `start..pos()`.
    fn pos(&mut self) -> usize {
        match self.chars.peek() {
            Some(&(offset, _)) => offset,
            None => self.line.len(),
        }
    }

    /// The next character, left unread.
    fn peek(&mut self) -> Option<char> {
        let &(_, c) = self.chars.peek()?;
        Some(c)
    }

    /// Reads the next character.
    fn bump(&mut self) -> Option<char> {
        let (_, c) = self.chars.next()?;
        Some(c)
    }

    /// Consumes the next character if it is `c`.
    fn eat(&mut self, c: char) -> bool {
        self.chars.next_if(|&(_, next)| next == c).is_some()
    }

    /// Consumes `text` if the line continues with it, for the one
    /// two-character token, `:-`.
    fn eat_str(&mut self, text: &str) -> bool {
        let start = self.pos();
        if !self.line[start..].starts_with(text) {
            return false;
        }
        for _ in text.chars() {
            self.bump();
        }
        true
    }

    /// Skips blanks; whether there were any.
    fn skip_blanks(&mut self) -> bool {
        let mut skipped = false;
        while self.chars.next_if(|&(_, c)| c.is_whitespace()).is_some() {
            skipped = true;
        }
        skipped
    }

    /// The line as an entry, or `None` when it is text.
    fn line(mut self) -> Result<Option<EnvEntry>, ParseErrorKind> {
        if self.eat('#') {
            let state = EnvEntryState::Commented {
                space_after_hash: self.eat(' '),
            };
            // A commented entry that does not read is prose, not an error.
            let Ok(entry) = self.entry(state) else {
                return Ok(None);
            };
            return Ok(Some(entry));
        }
        self.skip_blanks();
        if matches!(self.peek(), None | Some('#')) {
            return Ok(None);
        }
        Ok(Some(self.entry(EnvEntryState::Active)?))
    }

    /// `key = value`, after `state`'s prefix and, for an active entry, its
    /// leading blanks. Only an active entry allows blanks around its key.
    fn entry(mut self, state: EnvEntryState) -> Result<EnvEntry, ParseErrorKind> {
        let Some(key) = self.key() else {
            return Err(self.entry_error(state));
        };
        if state == EnvEntryState::Active {
            self.skip_blanks();
        }
        if !self.eat('=') {
            return Err(self.entry_error(state));
        }
        let raw = self.raw.to_owned();
        let (value, span) = self.value()?;
        Ok(EnvEntry {
            key,
            value,
            raw,
            span,
            state,
        })
    }

    /// `[A-Za-z_][A-Za-z0-9_]*`, or `None` when what is here is not one.
    fn key(&mut self) -> Option<EnvKey> {
        let start = self.pos();
        while self
            .chars
            .next_if(|&(_, c)| c.is_ascii_alphanumeric() || c == '_')
            .is_some()
        {}
        let end = self.pos();
        EnvKey::new(&self.line[start..end])
    }

    /// Why the line does not start with `KEY=`, quoting what it has instead.
    fn entry_error(&self, state: EnvEntryState) -> ParseErrorKind {
        let content = &self.line[state.prefix().len()..];
        let Some((name, _)) = content.split_once('=') else {
            return ParseErrorKind::MissingEquals(content.to_owned());
        };
        ParseErrorKind::InvalidKey(name.trim().to_owned())
    }

    /// Everything after `=`: the value and where it sits in the line, quotes
    /// included.
    fn value(mut self) -> Result<(EnvValue, Range<usize>), ValueError> {
        let after_eq = self.pos();
        let blank = self.skip_blanks();
        let start = self.pos();
        let value = match self.peek() {
            Some('"') => {
                self.bump();
                self.double_quoted()?
            }
            Some('\'') => {
                self.bump();
                self.single_quoted()?
            }
            _ => {
                return self.unquoted(after_eq, blank);
            }
        };
        let span = start..self.pos();
        self.after_quote()?;
        Ok((value, span))
    }

    /// `"..."`: escapes and references. The opening `"` is already read.
    fn double_quoted(&mut self) -> Result<EnvValue, ValueError> {
        let mut value = EnvValue::default();
        loop {
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
                Some('$') => {
                    self.dollar(&mut value, true)?;
                }
                Some(c) => {
                    value.push_char(c);
                }
            }
        }
    }

    /// `'...'`: everything literal. The opening `'` is already read.
    fn single_quoted(&mut self) -> Result<EnvValue, ValueError> {
        let start = self.pos();
        loop {
            match self.bump() {
                None => {
                    return Err(ValueError::UnclosedQuote);
                }
                Some('\'') => {
                    break;
                }
                Some(_) => {}
            }
        }
        // The closing quote is one byte.
        let end = self.pos() - 1;
        Ok(EnvValue::literal(self.line[start..end].to_owned()))
    }

    /// After a closing quote only blanks may follow, or blanks and a comment.
    fn after_quote(mut self) -> Result<(), ValueError> {
        let blank = self.skip_blanks();
        match self.peek() {
            None => Ok(()),
            Some('#') if blank => Ok(()),
            Some(_) => {
                let rest = &self.line[self.pos()..];
                Err(ValueError::TextAfterQuote(rest.to_owned()))
            }
        }
    }

    /// Unquoted: literal text and references, no escapes. The blanks after
    /// `=` (`after_eq`) are already skipped, and `blank` says whether there
    /// were any. Blanks around the value are not part of it, and a `#` after
    /// a blank starts a comment. An empty value sits right after `=`, so one
    /// written there later is not glued to a `#`.
    fn unquoted(
        mut self,
        after_eq: usize,
        blank: bool,
    ) -> Result<(EnvValue, Range<usize>), ValueError> {
        let mut value = EnvValue::default();
        let mut span = after_eq..after_eq;
        let mut after_blank = blank;
        while let Some(c) = self.peek() {
            if c == '#' && after_blank {
                break;
            }
            let at = self.pos();
            self.bump();
            after_blank = c.is_whitespace();
            if after_blank {
                continue;
            }
            // More of the value follows, so the blanks since its last
            // character are inside it; before its first one they are not.
            if !span.is_empty() {
                value.push_str(&self.line[span.end..at]);
            }
            match c {
                '$' => {
                    self.dollar(&mut value, false)?;
                }
                _ => {
                    value.push_char(c);
                }
            }
            if span.is_empty() {
                span.start = at;
            }
            span.end = self.pos();
        }
        Ok((value, span))
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

    /// What follows a `$`, appended to `value`: `${...}`, `$(...)`, or
    /// nothing, and then the `$` is just text. Inside `"..."` (`quoted`), a
    /// `"` before the closing `}` or `)` means it was never closed.
    fn dollar(&mut self, value: &mut EnvValue, quoted: bool) -> Result<(), ValueError> {
        if self.eat('{') {
            let (key, default) = self.reference(quoted)?;
            value.push_ref(key, default);
            return Ok(());
        }
        if self.eat('(') {
            value.push_call(self.call(quoted)?);
            return Ok(());
        }
        value.push_char('$');
        Ok(())
    }

    /// `NAME}` or `NAME:-default}`. The `${` is already read.
    fn reference(&mut self, quoted: bool) -> Result<(EnvKey, Option<String>), ValueError> {
        let start = self.pos();
        let Some(key) = self.key() else {
            return Err(self.invalid_reference(start, quoted));
        };
        if self.eat('}') {
            return Ok((key, None));
        }
        if self.eat_str(":-") {
            let default = self.default(quoted)?;
            return Ok((key, Some(default)));
        }
        Err(self.invalid_reference(start, quoted))
    }

    /// A reference's default, up to the closing `}`, which is consumed. Plain
    /// text: inside `"..."` it takes escapes.
    fn default(&mut self, quoted: bool) -> Result<String, ValueError> {
        let mut text = String::new();
        loop {
            match self.bump() {
                None => {
                    return Err(ValueError::UnclosedRef);
                }
                Some('}') => {
                    return Ok(text);
                }
                Some('"') if quoted => {
                    return Err(ValueError::UnclosedRef);
                }
                Some('\\') if quoted => {
                    text.push(self.escape()?);
                }
                Some('$') => {
                    if matches!(self.peek(), Some('{' | '(')) {
                        return Err(ValueError::NestedRef);
                    }
                    text.push('$');
                }
                Some(c) => {
                    text.push(c);
                }
            }
        }
    }

    /// Why the reference that started at `start` is not `NAME}` or
    /// `NAME:-default}`, quoting it up to its `}`.
    fn invalid_reference(&self, start: usize, quoted: bool) -> ValueError {
        let text = &self.line[start..];
        for (offset, c) in text.char_indices() {
            match c {
                '}' => {
                    return ValueError::InvalidRef(text[..offset].to_owned());
                }
                '"' if quoted => {
                    return ValueError::UnclosedRef;
                }
                _ => {}
            }
        }
        ValueError::UnclosedRef
    }

    /// `name args)`, read like a shell command: words up to the `)`, which
    /// is consumed. The `$(` is already read.
    fn call(&mut self, quoted: bool) -> Result<EnvCall, ValueError> {
        let start = self.pos();
        loop {
            match self.bump() {
                None => {
                    return Err(ValueError::UnclosedCall);
                }
                Some(')') => {
                    break;
                }
                Some('"') if quoted => {
                    return Err(ValueError::UnclosedCall);
                }
                Some(_) => {}
            }
        }
        // The closing `)` is one byte.
        let end = self.pos() - 1;
        let text = &self.line[start..end];
        let invalid = || ValueError::InvalidCall(text.to_owned());
        let mut words = text.split_whitespace();
        let Some(function) = words.next() else {
            return Err(invalid());
        };
        if function != "rand" {
            return Err(ValueError::UnknownFunction(function.to_owned()));
        }
        let Some(len) = words.next() else {
            return Err(invalid());
        };
        if words.next().is_some() {
            return Err(invalid());
        }
        let Ok(len) = len.parse::<usize>() else {
            return Err(invalid());
        };
        if !(1..=MAX_RAND_LEN).contains(&len) {
            return Err(invalid());
        }
        Ok(EnvCall::Rand { len })
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
        read_value(text).unwrap()
    }

    /// Reads `text` as what follows `=`.
    fn read_value(text: &str) -> Result<EnvValue, ValueError> {
        Ok(Cursor::new(text).value()?.0)
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

        // A `#` inside `${...}` is text, not a comment.
        let mut spaced = EnvValue::default();
        spaced.push_ref(key("X"), Some("a #b".to_owned()));
        assert_eq!(value("${X:-a #b}"), spaced);

        let mut secret = EnvValue::default();
        secret.push_call(EnvCall::Rand { len: 32 });
        assert_eq!(value("$(rand 32)"), secret);
        assert_eq!(value("$( rand  32 )"), secret);

        // A `$` followed by neither `{` nor `(` is text.
        assert_eq!(value("a$b$"), lit("a$b$"));
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
            ("  value# # comment", "value#"),
            ("a \t b  ", "a \t b"),
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
            ("${A B}", "invalid reference `${A B}`"),
            ("${rand(8)}", "invalid reference `${rand(8)}`"),
            (
                "${B:-${C}}",
                "a default cannot contain `${...}` or `$(...)`",
            ),
            (
                "${B:-$(rand 8)}",
                "a default cannot contain `${...}` or `$(...)`",
            ),
            ("$(rand 8", "unclosed `$(`"),
            ("$(nope 1)", "unknown function `nope` (only `rand` exists)"),
            (
                "$(rand 0)",
                "invalid call `$(rand 0)` (expected `$(rand n)` with n from 1 to 1024)",
            ),
            (
                "$(rand)",
                "invalid call `$(rand)` (expected `$(rand n)` with n from 1 to 1024)",
            ),
            (
                "$(rand 8 9)",
                "invalid call `$(rand 8 9)` (expected `$(rand n)` with n from 1 to 1024)",
            ),
        ] {
            let error = read_value(text).unwrap_err();
            assert_eq!(error.to_string(), expected, "{text:?}");
        }
    }
}
