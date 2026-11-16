//! Writes the final `.env`: the template's lines with values swapped in,
//! then the keys the template does not have.

use std::collections::HashSet;

use crate::key::Key;
use crate::merge::Merged;
use crate::parse::{EnvFile, Line};
use crate::value::Value;

/// A line whose value did not change comes back byte for byte; a changed
/// one has only its value replaced, keeping spacing and inline comment.
pub fn render(template: &EnvFile, merged: &Merged) -> String {
    let mut out = String::new();
    let mut in_template = HashSet::new();
    for line in &template.lines {
        match line {
            Line::Verbatim(raw) => out.push_str(raw),
            Line::Entry {
                key,
                value,
                raw,
                span,
            } => {
                in_template.insert(key);
                let final_value = merged.get(key).unwrap_or(value);
                if final_value == value {
                    out.push_str(raw);
                } else {
                    out.push_str(&raw[..span.start]);
                    final_value.write_env(&mut out);
                    out.push_str(&raw[span.end..]);
                }
            }
        }
    }
    let mut extra = merged
        .iter()
        .filter(|(key, _)| !in_template.contains(key))
        .peekable();
    if extra.peek().is_some() && !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for (key, value) in extra {
        push_entry(&mut out, key, value);
    }
    out
}

fn push_entry(out: &mut String, key: &Key, value: &Value) {
    out.push_str(key.as_str());
    out.push('=');
    value.write_env(out);
    out.push('\n');
}
