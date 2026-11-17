//! Builds `.env` files from templates, overrides and command-line values.
//!
//! The library never touches the disk or the process environment: callers
//! hand a [`Generator`] file contents and values, and get the output text
//! back.

mod env;
mod generator;
mod parse;
mod rand;
mod render;

pub use env::EnvKey;
pub use generator::{Generator, GeneratorError, Source};
pub use parse::{ParseError, ParseErrorKind, ValueError};
pub use rand::{Seed, new_seed_text};
