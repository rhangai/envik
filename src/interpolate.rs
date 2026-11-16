//! Expands `${NAME}` and `${NAME:-default}` against the final values, in
//! whatever order keys reference each other.

use std::collections::HashMap;
use std::fmt;

use crate::key::Key;
use crate::merge::Merged;
use crate::rand::Seed;
use crate::value::{Call, Part, Value};

/// A reference that could not be expanded.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InterpolateError {
    /// `key`'s value references `name`, which no source sets, with no default.
    #[error("`{key}`: `${{{name}}}` is not set")]
    Missing { key: Key, name: Key },
    /// `key`'s value calls `rand`, and no seed was given.
    #[error("`{key}`: `${{rand(...)}}` needs a seed (--seed-value, --seed-file or --seed-env)")]
    NoSeed { key: Key },
    /// Keys that reference each other in a loop; the first is repeated last.
    #[error("circular reference: {}", Chain(.chain))]
    Cycle { chain: Vec<Key> },
}

struct Chain<'a>(&'a [Key]);

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
/// empty; `${rand(n)}` needs `seed`. Returns
/// every missing reference and every cycle, each once: a key that fails only
/// because a key it references failed adds nothing.
pub fn interpolate(
    merged: &Merged<Value>,
    seed: Option<&Seed>,
) -> Result<Merged<String>, Vec<InterpolateError>> {
    let mut resolver = Resolver {
        merged,
        seed,
        state: HashMap::new(),
        stack: Vec::new(),
        errors: Vec::new(),
    };
    let mut resolved = Merged::default();
    for (key, value) in merged.iter() {
        if let Some(text) = resolver.resolve(key, value) {
            resolved.set(key, text);
        }
    }
    if resolver.errors.is_empty() {
        Ok(resolved)
    } else {
        Err(resolver.errors)
    }
}

enum State {
    /// On the current path: meeting it again closes a cycle.
    Visiting,
    Done(String),
    Failed,
}

struct Resolver<'a> {
    merged: &'a Merged<Value>,
    seed: Option<&'a Seed>,
    state: HashMap<&'a Key, State>,
    /// The keys being resolved, outermost first, to name a cycle's chain.
    stack: Vec<&'a Key>,
    errors: Vec<InterpolateError>,
}

impl<'a> Resolver<'a> {
    /// `key`'s expanded value, or `None` if it, or something it references,
    /// failed. `value` is `key`'s value in `merged`.
    fn resolve(&mut self, key: &'a Key, value: &'a Value) -> Option<String> {
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
    fn expand_value(&mut self, key: &'a Key, value: &'a Value) -> Option<String> {
        let mut text = String::new();
        let mut ok = true;
        let mut calls = 0;
        let mut no_seed_reported = false;
        for part in value.parts() {
            match part {
                Part::Literal(literal) => {
                    text.push_str(literal);
                }
                Part::Ref { name, default } => {
                    let Some(expanded) = self.expand(key, name, default.as_deref()) else {
                        ok = false;
                        continue;
                    };
                    text.push_str(&expanded);
                }
                Part::Call(Call::Rand { len }) => {
                    let index = calls;
                    calls += 1;
                    let Some(seed) = self.seed else {
                        ok = false;
                        // One error per key, however many calls it has.
                        if !no_seed_reported {
                            self.errors
                                .push(InterpolateError::NoSeed { key: key.clone() });
                            no_seed_reported = true;
                        }
                        continue;
                    };
                    text.push_str(&seed.rand(key, index, *len));
                }
            }
        }
        if !ok {
            return None;
        }
        Some(text)
    }

    /// One `${name}` or `${name:-default}` inside `key`'s value.
    fn expand(&mut self, key: &Key, name: &'a Key, default: Option<&str>) -> Option<String> {
        let Some(value) = self.merged.get(name) else {
            // Unset: the default, or an error.
            if let Some(default) = default {
                return Some(default.to_owned());
            }
            self.errors.push(InterpolateError::Missing {
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
    fn close_cycle(&mut self, key: &'a Key) {
        let start = self
            .stack
            .iter()
            .position(|on_path| *on_path == key)
            .expect("a key being visited is on the stack");
        let cycle = &self.stack[start..];
        let mut chain: Vec<Key> = cycle.iter().map(|member| (*member).clone()).collect();
        chain.push(key.clone());
        for member in cycle {
            self.state.insert(member, State::Failed);
        }
        self.errors.push(InterpolateError::Cycle { chain });
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Parses `src` as one file, applies `overrides` (`-e`) and interpolates;
    /// returns every key with its final text, in order.
    fn run(src: &str, overrides: &[(&str, &str)]) -> Vec<(String, String)> {
        let file = crate::parse::parse(src, Path::new("test.env")).unwrap();
        let overrides: Vec<(Key, String)> = overrides
            .iter()
            .map(|(name, text)| (Key::new(name).unwrap(), (*text).to_owned()))
            .collect();
        let merged = crate::merge::merge(&[file], &overrides);
        let resolved = interpolate(&merged, None).unwrap();
        merged
            .iter()
            .map(|(key, _)| {
                let text = resolved.get(key).unwrap();
                (key.as_str().to_owned(), text.clone())
            })
            .collect()
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
}
