//! Turns the inputs of one generation into the text of a `.env`.
//!
//! [`Generator::new`] parses the files; [`Generator::env_flag`],
//! [`Generator::env_override`] and [`Generator::seed`] add the values given
//! outside them, in any order; [`Generator::generate`] resolves every key and
//! writes the text. Later sources win: the files in order, then the env
//! flags, then the env overrides, which only replace keys that exist.
//!
//! # Implementation
//!
//! Resolving runs `${...}` and `$(...)` against the final values, whatever
//! order keys reference each other in: a depth-first walk ([`Resolver`]) with
//! each key visited once, so a cycle is met as a key still being visited.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::path::Path;

use crate::env::{EnvCall, EnvFile, EnvKey, EnvMap, EnvValue, EnvValuePart};
use crate::parse::{self, ParseError};
use crate::rand::Seed;
use crate::render;

/// One input file: its contents, and where it came from for error messages.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub origin: &'a Path,
    pub contents: &'a str,
}

/// Collects the inputs of one generation. Its files are always valid:
/// [`Generator::new`] refuses invalid ones.
#[derive(Debug)]
pub struct Generator {
    /// The first input, whose layout the output keeps.
    template: EnvFile,
    /// Each key's value from the files, then the env flags.
    values: EnvMap<EnvValue>,
    /// Applied by [`Generator::generate`], so they win over env flags added
    /// after them too.
    env_overrides: Vec<(EnvKey, OsString)>,
    /// `None` until [`Generator::seed`]: any `$(rand n)` is then an error.
    seed: Option<Seed>,
}

impl Generator {
    /// Parses every input, later ones winning; on failure, every invalid
    /// line across all of them. With no inputs, the output is just the env
    /// flags.
    pub fn new(inputs: &[Source<'_>]) -> Result<Self, Vec<ParseError>> {
        let mut files = Vec::with_capacity(inputs.len());
        let mut errors = Vec::new();
        for source in inputs {
            match parse::parse_file(source.contents, source.origin) {
                Ok(file) => files.push(file),
                Err(found) => errors.extend(found),
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut values = EnvMap::default();
        for (key, value) in files.iter().flat_map(EnvFile::entries) {
            values.set(key, value.clone());
        }
        let mut files = files.into_iter();
        let template = files.next().unwrap_or(EnvFile { lines: Vec::new() });
        Ok(Generator {
            template,
            values,
            env_overrides: Vec::new(),
            seed: None,
        })
    }

    /// `-e`: sets `key` whether or not the files have it, literally.
    pub fn env_flag(&mut self, key: EnvKey, value: String) {
        self.values.set(&key, EnvValue::literal(value));
    }

    /// Replaces `key`, literally, only if it exists: set by a file or an env
    /// flag, or offered as `# KEY=` by the template. Raw: only a value that
    /// is used must be UTF-8.
    pub fn env_override(&mut self, key: EnvKey, value: OsString) {
        self.env_overrides.push((key, value));
    }

    /// What `$(rand n)` derives from; without it, any call is an error.
    pub fn seed(&mut self, seed: Seed) {
        self.seed = Some(seed);
    }

    /// The `.env` text.
    pub fn generate(&self) -> Result<String, Vec<GeneratorError>> {
        let values = self.values_with_overrides()?;
        let resolved = resolve(&values, self.seed.as_ref())?;
        Ok(render::render(&self.template, &resolved))
    }

    /// The values with every env override that applies.
    fn values_with_overrides(&self) -> Result<EnvMap<EnvValue>, Vec<GeneratorError>> {
        let mut values = self.values.clone();
        let mut errors = Vec::new();
        for (key, raw) in &self.env_overrides {
            let exists = self.values.get(key).is_some() || self.template.offers(key);
            if !exists {
                continue;
            }
            let Some(text) = raw.to_str() else {
                errors.push(GeneratorError::OverrideNotUtf8 { key: key.clone() });
                continue;
            };
            values.set(key, EnvValue::literal(text.to_owned()));
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(values)
    }
}

/// One thing that kept a generation from finishing.
///
/// [`Generator::generate`] returns every one it found, from the first step
/// that failed: env overrides first, then references. A step that fails
/// stops the one after it, so a key failing only because of an earlier
/// error adds nothing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GeneratorError {
    /// An env override for a key that exists, with a value that is not
    /// UTF-8.
    #[error("`{key}`: its value from the environment is not valid UTF-8")]
    OverrideNotUtf8 { key: EnvKey },
    /// `key`'s value references `name`, which no source sets, with no default.
    #[error("`{key}`: `${{{name}}}` is not set")]
    Missing { key: EnvKey, name: EnvKey },
    /// `key`'s value calls `rand`, and no seed was given.
    #[error("`{key}`: `$(rand ...)` needs a seed")]
    NoSeed { key: EnvKey },
    /// `key`'s final value still has a `$(required)`: no source after the
    /// one that wrote it replaced it. `value` is that value, for context.
    #[error("`{key}` is required: `{value}`")]
    Required { key: EnvKey, value: EnvValue },
    /// Keys that reference each other in a loop; the first is repeated last.
    #[error("circular reference: {}", Chain(.chain))]
    Cycle { chain: Vec<EnvKey> },
}

/// A cycle's keys as `A → B → A`, for [`GeneratorError::Cycle`].
struct Chain<'a>(&'a [EnvKey]);

impl fmt::Display for Chain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, key) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(" → ")?;
            }
            write!(f, "{key}")?;
        }
        Ok(())
    }
}

/// Every key's value with its references expanded and its calls run, in the
/// same order. `${NAME:-default}` takes `default` when `NAME` is unset or
/// empty; `$(rand n)` needs `seed`. Returns every missing reference and
/// every cycle, each once: a key that fails only because a key it
/// references failed adds nothing.
fn resolve(
    values: &EnvMap<EnvValue>,
    seed: Option<&Seed>,
) -> Result<EnvMap<String>, Vec<GeneratorError>> {
    let mut resolver = Resolver {
        values,
        seed,
        state: HashMap::new(),
        stack: Vec::new(),
        errors: Vec::new(),
    };
    let mut resolved = EnvMap::default();
    for (key, value) in values.iter() {
        if let Some(text) = resolver.resolve(key, value) {
            resolved.set(key, text);
        }
    }
    if !resolver.errors.is_empty() {
        return Err(resolver.errors);
    }
    Ok(resolved)
}

enum State {
    /// On the current path: meeting it again closes a cycle.
    Visiting,
    /// Resolved to this text.
    Done(String),
    /// It, or something it references, failed; its error is already out.
    Failed,
}

struct Resolver<'a> {
    /// Every key's value, env overrides already applied.
    values: &'a EnvMap<EnvValue>,
    /// `None`: every `$(rand n)` is a [`GeneratorError::NoSeed`].
    seed: Option<&'a Seed>,
    /// Keys met so far; one with no entry has not been visited.
    state: HashMap<&'a EnvKey, State>,
    /// The keys being resolved, outermost first, to name a cycle's chain.
    stack: Vec<&'a EnvKey>,
    /// Every error found, each once.
    errors: Vec<GeneratorError>,
}

impl<'a> Resolver<'a> {
    /// `key`'s expanded value, or `None` if it, or something it references,
    /// failed. `value` is `key`'s value in `values`.
    fn resolve(&mut self, key: &'a EnvKey, value: &'a EnvValue) -> Option<String> {
        match self.state.get(key) {
            Some(State::Done(text)) => {
                return Some(text.clone());
            }
            Some(State::Failed) => {
                return None;
            }
            Some(State::Visiting) => {
                self.close_cycle(key);
                return None;
            }
            None => {
                // Not met yet: resolve it below.
            }
        }
        self.state.insert(key, State::Visiting);
        self.stack.push(key);
        let text = self.expand_value(key, value);
        self.stack.pop();
        match text {
            Some(text) => {
                self.state.insert(key, State::Done(text.clone()));
                Some(text)
            }
            None => {
                self.state.insert(key, State::Failed);
                None
            }
        }
    }

    /// `value`'s parts joined, with every reference expanded and every call
    /// run; `None` if any of them failed. Keeps going after a failure, so
    /// every error in the value is reported.
    fn expand_value(&mut self, key: &'a EnvKey, value: &'a EnvValue) -> Option<String> {
        let mut text = String::new();
        let mut ok = true;
        let mut calls = 0;
        let mut no_seed_reported = false;
        let mut required_reported = false;
        for part in value.parts() {
            match part {
                EnvValuePart::Literal(literal) => {
                    text.push_str(literal);
                }
                EnvValuePart::Ref { name, default } => {
                    let Some(expanded) = self.expand(key, name, default.as_deref()) else {
                        ok = false;
                        continue;
                    };
                    text.push_str(&expanded);
                }
                EnvValuePart::Call(EnvCall::Rand { len }) => {
                    let index = calls;
                    calls += 1;
                    let Some(seed) = self.seed else {
                        ok = false;
                        // One error per key, however many calls it has.
                        if !no_seed_reported {
                            self.errors
                                .push(GeneratorError::NoSeed { key: key.clone() });
                            no_seed_reported = true;
                        }
                        continue;
                    };
                    text.push_str(&seed.rand(key, index, *len));
                }
                EnvValuePart::Call(EnvCall::Required) => {
                    ok = false;
                    // One error per key, however many it has.
                    if !required_reported {
                        self.errors.push(GeneratorError::Required {
                            key: key.clone(),
                            value: value.clone(),
                        });
                        required_reported = true;
                    }
                }
            }
        }
        if !ok {
            return None;
        }
        Some(text)
    }

    /// One `${name}` or `${name:-default}` inside `key`'s value.
    fn expand(&mut self, key: &EnvKey, name: &'a EnvKey, default: Option<&str>) -> Option<String> {
        let Some(value) = self.values.get(name) else {
            // Unset: the default, or an error.
            if let Some(default) = default {
                return Some(default.to_owned());
            }
            self.errors.push(GeneratorError::Missing {
                key: key.clone(),
                name: name.clone(),
            });
            return None;
        };
        let text = self.resolve(name, value)?;
        // Empty counts as unset for `:-`, as in the shell.
        match default {
            Some(default) if text.is_empty() => Some(default.to_owned()),
            _ => Some(text),
        }
    }

    /// `key` was met again while still being resolved: everything on the path
    /// from it is in the loop.
    fn close_cycle(&mut self, key: &'a EnvKey) {
        let start = self
            .stack
            .iter()
            .position(|on_path| *on_path == key)
            .expect("a key being visited is on the stack");
        let cycle = &self.stack[start..];
        let mut chain: Vec<EnvKey> = cycle.iter().map(|member| (*member).clone()).collect();
        chain.push(key.clone());
        for member in cycle {
            self.state.insert(member, State::Failed);
        }
        self.errors.push(GeneratorError::Cycle { chain });
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn key(name: &str) -> EnvKey {
        EnvKey::new(name).unwrap()
    }

    /// A generator over `sources`, the first being the template.
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

    /// Every key in order, with its value after the env overrides, as
    /// literal text.
    fn literals(generator: &Generator) -> Vec<(String, String)> {
        let values = generator.values_with_overrides().unwrap();
        values
            .iter()
            .map(|(key, value)| (key.to_string(), value.as_literal().unwrap().to_owned()))
            .collect()
    }

    /// Every key in order, with its resolved text.
    fn texts(generator: &Generator) -> Vec<(String, String)> {
        let values = generator.values_with_overrides().unwrap();
        let resolved = resolve(&values, generator.seed.as_ref()).unwrap();
        resolved
            .iter()
            .map(|(key, text)| (key.to_string(), text.clone()))
            .collect()
    }

    /// `src` as the only file, with `env_flags` (`-e`), resolved.
    fn run(src: &str, env_flags: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut generator = generator(&[src]);
        for (name, text) in env_flags {
            generator.env_flag(key(name), (*text).to_owned());
        }
        texts(&generator)
    }

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn expands_with_final_values() {
        let src = "HOST=localhost\nPORT=3000\nURL=http://${HOST}:${PORT}\nD=${X:-padrao}\n";

        assert_eq!(
            run(src, &[("PORT", "445")]),
            kv(&[
                ("HOST", "localhost"),
                ("PORT", "445"),
                ("URL", "http://localhost:445"),
                ("D", "padrao"),
            ]),
        );
        assert_eq!(
            run(src, &[("X", "valor")]),
            kv(&[
                ("HOST", "localhost"),
                ("PORT", "3000"),
                ("URL", "http://localhost:3000"),
                ("D", "valor"),
                ("X", "valor"),
            ]),
        );
        assert_eq!(
            run(src, &[("X", "")]),
            kv(&[
                ("HOST", "localhost"),
                ("PORT", "3000"),
                ("URL", "http://localhost:3000"),
                ("D", "padrao"),
                ("X", ""),
            ]),
        );
        // Empty in a file, through a reference, counts as empty too.
        assert_eq!(
            run("D=${X:-padrao}\nE=\nX=${E}\n", &[]),
            kv(&[("D", "padrao"), ("E", ""), ("X", "")]),
        );
    }

    #[test]
    fn resolves_chain_out_of_order() {
        let src = "URL=https://${BASE}\nPORT=123\nBASE=site:${PORT}\n";

        assert_eq!(
            run(src, &[]),
            kv(&[
                ("URL", "https://site:123"),
                ("PORT", "123"),
                ("BASE", "site:123"),
            ]),
        );
        assert_eq!(
            run(src, &[("PORT", "445")]),
            kv(&[
                ("URL", "https://site:445"),
                ("PORT", "445"),
                ("BASE", "site:445"),
            ]),
        );
    }

    #[test]
    fn env_override_only_existing_keys() {
        let mut generator = generator(&["A=1\nB=2\n", "C=3\n"]);
        generator.env_override(key("A"), "oa".into());
        generator.env_override(key("C"), "oc".into());
        generator.env_override(key("NEW"), "on".into());
        // Added before the env flag it replaces: still wins.
        generator.env_override(key("E"), "oe".into());
        generator.env_flag(key("E"), "flag".to_owned());

        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        assert_eq!(
            literals(&generator),
            [
                pair("A", "oa"),
                pair("B", "2"),
                pair("C", "oc"),
                pair("E", "oe")
            ],
        );
    }

    #[cfg(unix)]
    #[test]
    fn env_override_non_utf8_only_when_used() {
        use std::os::unix::ffi::OsStringExt;

        let not_utf8 = || OsString::from_vec(vec![b'x', 0xff, b'y']);

        let mut unused = generator(&["A=1\n"]);
        unused.env_override(key("NOPE"), not_utf8());
        assert_eq!(literals(&unused), [("A".to_owned(), "1".to_owned())]);

        let mut used = generator(&["A=1\n"]);
        used.env_override(key("A"), not_utf8());
        let errors = used.values_with_overrides().unwrap_err();
        let keys: Vec<&str> = errors
            .iter()
            .map(|error| match error {
                GeneratorError::OverrideNotUtf8 { key } => key.as_str(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(keys, ["A"]);
    }

    #[test]
    fn env_override_is_literal_and_referenced() {
        let mut generator = generator(&["X=old\nY=why\nURL=http://${X}\n"]);
        generator.env_override(key("X"), "new${Y}".into());

        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        assert_eq!(
            texts(&generator),
            [
                pair("X", "new${Y}"),
                pair("Y", "why"),
                pair("URL", "http://new${Y}")
            ],
        );
    }

    /// `generate`'s errors, as text.
    fn errors(generator: &Generator) -> Vec<String> {
        let errors = generator.generate().unwrap_err();
        errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn required_is_an_error_until_replaced() {
        let template = concat!(
            "DEBUG=$(required)\n",
            "URL=https://$(required)/\n",
            "KEY=$(required) get one at https://example.com/keys\n",
            "FULL=${URL}x\n",
            "TWO=$(required)-$(required)\n",
        );

        // One error per key, with its whole template value; `FULL` fails
        // only because `URL` did, so it adds nothing.
        assert_eq!(
            errors(&generator(&[template])),
            [
                "`DEBUG` is required: `$(required)`",
                "`URL` is required: `https://$(required)/`",
                "`KEY` is required: `$(required) get one at https://example.com/keys`",
                "`TWO` is required: `$(required)-$(required)`",
            ],
        );

        // A later file, an env flag and an env override each replace one.
        let mut generator = generator(&[template, "URL=https://example.com/\n"]);
        generator.env_flag(key("DEBUG"), "1".to_owned());
        generator.env_flag(key("KEY"), "k".to_owned());
        generator.env_override(key("TWO"), "x".into());
        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        assert_eq!(
            texts(&generator),
            [
                pair("DEBUG", "1"),
                pair("URL", "https://example.com/"),
                pair("KEY", "k"),
                pair("FULL", "https://example.com/x"),
                pair("TWO", "x"),
            ],
        );
    }

    #[test]
    fn required_on_an_optional_line_is_dropped() {
        let generator = generator(&["A=1\n# OPT=$(required)\n"]);
        assert_eq!(generator.generate().unwrap(), "A=1\n");
    }
}
