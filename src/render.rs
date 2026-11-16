//! Writes the final `.env`: the template's lines with values swapped in,
//! then the keys the template does not have.

use std::collections::HashSet;

use crate::key::Key;
use crate::merge::Merged;
use crate::parse::{Entry, EnvFile, Line};
use crate::value::Value;

/// A line whose value did not change comes back byte for byte; a changed
/// one has only its value replaced, keeping spacing and inline comment.
///
/// A commented `# KEY=` line becomes active in place when some source sets
/// `KEY` and the template has no active `KEY` line; only the first such line
/// per key does. When nothing sets `KEY`, the line is dropped.
pub fn render(template: &EnvFile, merged: &Merged) -> String {
    // Keys that already have their line: every active one, then each
    // commented one as it is turned on.
    let mut placed: HashSet<&Key> = template.entries().map(|(key, _)| key).collect();
    let mut out = String::new();
    for line in &template.lines {
        match line {
            Line::Verbatim(raw) => out.push_str(raw),
            Line::Entry(entry) => push_with_final_value(&mut out, entry, merged),
            Line::Commented {
                space_after_hash,
                entry,
            } => match merged.get(&entry.key) {
                None => {}
                Some(_) if placed.contains(&entry.key) => {
                    out.push_str(if *space_after_hash { "# " } else { "#" });
                    out.push_str(&entry.raw);
                }
                Some(_) => {
                    placed.insert(&entry.key);
                    push_with_final_value(&mut out, entry, merged);
                }
            },
        }
    }
    let mut extra = merged
        .iter()
        .filter(|(key, _)| !placed.contains(key))
        .peekable();
    if extra.peek().is_some() && !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for (key, value) in extra {
        push_entry(&mut out, key, value);
    }
    out
}

/// `entry`'s line with its value replaced by the final one, or untouched when
/// they are equal.
fn push_with_final_value(out: &mut String, entry: &Entry, merged: &Merged) {
    let final_value = merged.get(&entry.key).unwrap_or(&entry.value);
    if *final_value == entry.value {
        out.push_str(&entry.raw);
    } else {
        out.push_str(&entry.raw[..entry.span.start]);
        final_value.write_env(out);
        out.push_str(&entry.raw[entry.span.end..]);
    }
}

fn push_entry(out: &mut String, key: &Key, value: &Value) {
    out.push_str(key.as_str());
    out.push('=');
    value.write_env(out);
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::merge::merge;
    use crate::parse::parse;

    /// Renders `template`, with `template` and then `others` as the input
    /// files and `overrides` as the `-e` values.
    fn render_text(template: &str, others: &[&str], overrides: &[(&str, &str)]) -> String {
        let read = |src: &str| parse(src, Path::new("test.env")).unwrap();
        let files: Vec<EnvFile> = std::iter::once(template)
            .chain(others.iter().copied())
            .map(read)
            .collect();
        let overrides: Vec<(Key, String)> = overrides
            .iter()
            .map(|(key, value)| (Key::new(key).unwrap(), (*value).to_owned()))
            .collect();
        render(&read(template), &merge(&files, &overrides))
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
            "# PORT padrão é 3000\n#  PORT=1\nHOST=x\nPORT=1\n",
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
}
