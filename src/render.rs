//! Writes the final `.env`: the template's lines with values swapped in,
//! then the keys the template does not have.

use std::collections::HashSet;

use crate::env::{EnvEntry, EnvEntryState, EnvFile, EnvFileLine, EnvKey, EnvMap};

/// A line whose value did not change comes back byte for byte; a changed
/// one has only its value replaced, keeping spacing and inline comment.
///
/// A commented `# KEY=` line becomes active in place when some source sets
/// `KEY` and the template has no active `KEY` line; only the first such line
/// per key does. When nothing sets `KEY`, the line is dropped.
pub fn render(template: &EnvFile, resolved: &EnvMap<String>) -> String {
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
        // Every active line of the template is a value, so it is always set.
        let Some(text) = resolved.get(&entry.key) else {
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
    for (key, text) in resolved.iter() {
        if placed.contains(key) {
            continue;
        }
        if !extras_started {
            push_extras_header(&mut out);
            extras_started = true;
        }
        push_entry(&mut out, key, text);
    }
    // A template whose last line has no newline still gives a text file
    // that ends with one.
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
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

/// A `KEY=value` line for a key the template does not have.
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
/// neighbours: no blank, `#`, quote, `\` or `$`, and none a shell sourcing
/// the file would act on, such as `&` or `~`. `?` is safe: a shell does not
/// expand it in an assignment.
fn is_safe_bare(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_-./:@%+,=?".contains(c)
}

/// `text` with what is special inside `"..."` escaped.
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

    use crate::env::EnvValue;
    use crate::parse::parse_file;
    use crate::{EnvKey, Generator, Source};

    fn generator(sources: &[&str]) -> Generator {
        let sources: Vec<Source<'_>> = sources
            .iter()
            .map(|contents| Source {
                origin: Path::new("test.env"),
                contents,
            })
            .collect();
        Generator::new(&sources).unwrap()
    }

    /// Renders `template`, with `template` and then `others` as the input
    /// files and `env_flags` as the `-e` values.
    fn render_text(template: &str, others: &[&str], env_flags: &[(&str, &str)]) -> String {
        let sources: Vec<&str> = std::iter::once(template)
            .chain(others.iter().copied())
            .collect();
        let mut generator = generator(&sources);
        for (key, value) in env_flags {
            generator.env_flag(EnvKey::new(key).unwrap(), (*value).to_owned());
        }
        generator.generate().unwrap()
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
            "http://a/b?client_id=rh",
            "?a=1&b=2",
            "~/x",
        ];
        for text in texts {
            let written = render_text("", &[], &[("A", text)]);
            let file = parse_file(&written, Path::new("out.env")).unwrap();
            let values: Vec<_> = file.entries().map(|(_, value)| value).collect();
            assert_eq!(values, [&EnvValue::literal(text.to_owned())], "{written:?}");
        }
    }

    #[test]
    fn env_override_turns_on_optional_line() {
        let mut generator = generator(&["A=1\n# OPT=default\nB=2\n"]);
        generator.env_override(EnvKey::new("OPT").unwrap(), "on".into());

        assert_eq!(generator.generate().unwrap(), "A=1\nOPT=on\nB=2\n");
    }

    #[test]
    fn question_mark_stays_bare() {
        assert_eq!(
            render_text("", &[], &[("URL", "http://a/b?client_id=rh")]),
            "URL=http://a/b?client_id=rh\n",
        );
        // `&` would end the command in a shell, so it is still quoted.
        assert_eq!(
            render_text("", &[], &[("URL", "http://a/b?a=1&b=2")]),
            "URL='http://a/b?a=1&b=2'\n",
        );
    }

    #[test]
    fn output_ends_with_newline() {
        assert_eq!(render_text("A=1\nB=2", &[], &[]), "A=1\nB=2\n");
        assert_eq!(render_text("A=1\nB=2", &[], &[("B", "3")]), "A=1\nB=3\n");
        assert_eq!(render_text("A=1\n", &[], &[]), "A=1\n");
        assert_eq!(render_text("", &[], &[]), "");
    }
}
