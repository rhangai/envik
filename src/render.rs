//! Writes the final `.env`: the template's lines with values swapped in,
//! then the keys the template does not have.

use std::collections::HashSet;

use crate::env::{EnvEntry, EnvEntryState, EnvFile, EnvFileLine, EnvKey};
use crate::merge::Merged;

/// A line whose value did not change comes back byte for byte; a changed
/// one has only its value replaced, keeping spacing and inline comment.
///
/// A commented `# KEY=` line becomes active in place when some source sets
/// `KEY` and the template has no active `KEY` line; only the first such line
/// per key does. When nothing sets `KEY`, the line is dropped.
pub fn render(template: &EnvFile, merged: &Merged<String>) -> String {
    // Keys that already have their line: every active one, then each
    // commented one as it is turned on.
    let mut placed: HashSet<&EnvKey> = template.entries().map(|(key, _)| key).collect();
    let mut out = String::new();
    for line in &template.lines {
        let entry = match line {
            EnvFileLine::Text(raw) => {
                out.push_str(raw);
                continue;
            }
            EnvFileLine::Entry(entry) => entry,
        };
        // Merging reads the template first, so an active line is always set.
        let Some(text) = merged.get(&entry.key) else {
            if entry.state == EnvEntryState::Active {
                out.push_str(&entry.raw);
            }
            // A commented line nothing sets is left out.
            continue;
        };
        if entry.state != EnvEntryState::Active {
            if placed.contains(&entry.key) {
                // Already active elsewhere: the comment stays a comment.
                out.push_str(&entry.raw);
                continue;
            }
            placed.insert(&entry.key);
        }
        push_with_final_value(&mut out, entry, text);
    }
    let mut extras_started = false;
    for (key, text) in merged.iter() {
        if placed.contains(key) {
            continue;
        }
        if !extras_started {
            push_extras_header(&mut out);
            extras_started = true;
        }
        push_entry(&mut out, key, text);
    }
    out
}

/// Starts the keys the template does not have: a blank line, then
/// [`EXTRAS_COMMENT`], so they do not read as part of the template's last
/// block. Nothing when the template wrote nothing.
fn push_extras_header(out: &mut String) {
    if out.is_empty() {
        return;
    }
    // A template whose last line has no newline.
    if !out.ends_with('\n') {
        out.push('\n');
    }
    // One blank line, even if the template already ends with one.
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(EXTRAS_COMMENT);
}

/// The line above the keys the template does not have.
const EXTRAS_COMMENT: &str = "# ----\n";

/// `entry`'s line with its value replaced by `final_text`. It stays untouched
/// only when it holds no reference and already says that value: a `${...}`
/// line is always rewritten with what it expanded to.
fn push_with_final_value(out: &mut String, entry: &EnvEntry, final_text: &str) {
    // A commented line is turned on: its `#` or `# ` goes.
    let start = entry.state.prefix().len();
    if entry.value.as_literal() == Some(final_text) {
        out.push_str(&entry.raw[start..]);
        return;
    }
    out.push_str(&entry.raw[start..entry.span.start]);
    write_literal(final_text, out);
    out.push_str(&entry.raw[entry.span.end..]);
}

fn push_entry(out: &mut String, key: &EnvKey, text: &str) {
    out.push_str(key.as_str());
    out.push('=');
    write_literal(text, out);
    out.push('\n');
}

/// Appends `text` as it goes after `KEY=`, so that it reads back as the same
/// literal: bare when every character is safe; otherwise in single quotes,
/// where nothing is special; and in double quotes with escapes only when it
/// holds a `'` or a newline, which single quotes cannot.
fn write_literal(text: &str, out: &mut String) {
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::env::EnvValue;
    use crate::merge::merge;
    use crate::parse::parse_file;

    /// Renders `template`, with `template` and then `others` as the input
    /// files and `overrides` as the `-e` values.
    fn render_text(template: &str, others: &[&str], overrides: &[(&str, &str)]) -> String {
        let read = |src: &str| parse_file(src, Path::new("test.env")).unwrap();
        let files: Vec<EnvFile> = std::iter::once(template)
            .chain(others.iter().copied())
            .map(read)
            .collect();
        let overrides: Vec<(EnvKey, String)> = overrides
            .iter()
            .map(|(key, value)| (EnvKey::new(key).unwrap(), (*value).to_owned()))
            .collect();
        let resolved = crate::interpolate::interpolate(&merge(&files, &overrides), None).unwrap();
        render(&read(template), &resolved)
    }

    #[test]
    fn undefined_commented_key_is_dropped() {
        assert_eq!(
            render_text("A=1\n# DEBUG=\n#DEBUG=1\n# DEBUG\nB=2\n", &[], &[]),
            "A=1\n# DEBUG\nB=2\n",
        );
    }

    #[test]
    fn free_text_comment_untouched() {
        assert_eq!(
            render_text(
                "# PORT padrão é 3000\n#  PORT=1\nHOST=x\n",
                &[],
                &[("PORT", "1")],
            ),
            "# PORT padrão é 3000\n#  PORT=1\nHOST=x\n\n# ----\nPORT=1\n",
        );
    }

    #[test]
    fn only_first_eligible_comment() {
        // `PORT` is active after its comment; `DEBUG` is only commented, and
        // set by a second input file.
        assert_eq!(
            render_text(
                "# PORT=8080\nPORT=3000\n# DEBUG=\n# DEBUG=true\n",
                &["DEBUG=1\n"],
                &[("PORT", "9000")],
            ),
            "# PORT=8080\nPORT=9000\nDEBUG=1\n# DEBUG=true\n",
        );
    }

    #[test]
    fn written_values_read_back() {
        // Every character that needs quoting, and literal text that looks
        // like a reference.
        let texts = [
            "",
            "plain",
            "a=b:c/d",
            "a b",
            " lead and trail ",
            "a#b",
            "a #b",
            "#",
            "x\"y",
            "it's",
            "c\\d",
            "\\",
            "$",
            "$HOME",
            "${X}",
            "a${X:-d}b",
            "a\nb",
            "\n",
            "tab\there",
            "ação",
        ];
        for text in texts {
            let written = render_text("", &[], &[("A", text)]);
            let file = parse_file(&written, Path::new("out.env")).unwrap();
            let values: Vec<_> = file.entries().map(|(_, value)| value).collect();
            assert_eq!(values, [&EnvValue::literal(text.to_owned())], "{written:?}");
        }
    }
}
