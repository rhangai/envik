//! Builds `.env` files from templates, overrides and command-line values.
//!
//! The library never touches the disk or the process environment: callers
//! hand it file contents and values, and get the output text back.

mod env;
mod interpolate;
mod merge;
mod parse;
mod rand;
mod render;

use std::fmt;
use std::path::Path;

pub use env::EnvKey;
pub use interpolate::InterpolateError;
pub use parse::{ParseError, ParseErrorKind, ValueError};
pub use rand::{Seed, new_seed_text};

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
/// inputs, the output is just the overrides. `seed` is what `${rand(n)}`
/// derives from; without it, any `rand` call is an error.
pub fn generate(
    inputs: &[Source<'_>],
    overrides: &[(EnvKey, String)],
    seed: Option<&Seed>,
) -> Result<String, Error> {
    let mut files = Vec::with_capacity(inputs.len());
    let mut errors = Vec::new();
    for source in inputs {
        match parse::parse_file(source.contents, source.origin) {
            Ok(file) => files.push(file),
            Err(found) => errors.extend(found),
        }
    }
    if !errors.is_empty() {
        return Err(Error::Parse(errors));
    }
    let merged = merge::merge(&files, overrides);
    let resolved = interpolate::interpolate(&merged, seed).map_err(Error::Interpolate)?;
    let empty = env::EnvFile { lines: Vec::new() };
    let template = files.first().unwrap_or(&empty);
    Ok(render::render(template, &resolved))
}
