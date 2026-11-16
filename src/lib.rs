//! Builds `.env` files from templates, overrides and command-line values.
//!
//! The library never touches the disk or the process environment: callers
//! hand it file contents and values, and get the output text back.

mod interpolate;
mod key;
mod merge;
mod parse;
mod render;
mod value;

use std::fmt;
use std::path::Path;

pub use interpolate::InterpolateError;
pub use key::Key;
pub use parse::{ParseError, ParseErrorKind};
pub use value::ValueError;

/// One input file: its contents, and where it came from for error messages.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub origin: &'a Path,
    pub contents: &'a str,
}

/// Any failure of [`generate`]. Each variant is one step, carrying every
/// error that step found (never empty); a step that fails stops the steps
/// after it, so their errors are never mixed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Invalid lines, across all inputs, in input order.
    #[error("{}", one_per_line(.0))]
    Parse(Vec<ParseError>),
    /// Missing references and cycles, across all keys.
    #[error("{}", one_per_line(.0))]
    Interpolate(Vec<InterpolateError>),
}

/// Joins errors one per line.
fn one_per_line<T: fmt::Display>(errors: &[T]) -> String {
    errors
        .iter()
        .map(T::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Merges `inputs` (later ones winning) and `overrides` (always winning) into
/// the text of a `.env`. The first input is the layout template; with no
/// inputs, the output is just the overrides.
pub fn generate(inputs: &[Source<'_>], overrides: &[(Key, String)]) -> Result<String, Error> {
    let mut files = Vec::with_capacity(inputs.len());
    let mut errors = Vec::new();
    for source in inputs {
        match parse::parse(source.contents, source.origin) {
            Ok(file) => files.push(file),
            Err(found) => errors.extend(found),
        }
    }
    if !errors.is_empty() {
        return Err(Error::Parse(errors));
    }
    let merged = merge::merge(&files, overrides);
    let resolved = interpolate::interpolate(&merged).map_err(Error::Interpolate)?;
    let empty = parse::EnvFile { lines: Vec::new() };
    let template = files.first().unwrap_or(&empty);
    Ok(render::render(template, &resolved))
}
