# AGENTS.md

envik is a Rust CLI (and library) that builds `.env` files: it merges input
files, `-e` values and environment overrides into one `.env`, keeping the
first file's layout, expanding `${VAR}` references and generating seeded
secrets with `$(rand n)`. The user-facing behaviour is in
[README.md](README.md); how the code fits together is in
[docs/architecture.md](docs/architecture.md); why things are the way they are
is in [docs/decisions.md](docs/decisions.md). Read the decision log before
proposing to change something it covers.

Docs, comments, commit messages and the README are in **English**.

## Checks

Run all of these before calling a change done; clippy must be clean, not
quiet:

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo doc --no-deps --document-private-items   # no broken intra-doc links
```

CI (`.github/workflows/ci.yml`) runs fmt, clippy with `-D warnings` and the
tests on pull requests and on pushes to `main`.

## Layout

```
src/lib.rs          re-exports only: Generator, GeneratorError, Source, EnvKey,
                    Seed, ParseError, ParseErrorKind, ValueError, new_seed_text
src/env.rs          the model: EnvKey, EnvValue, EnvFile, EnvEntry, EnvMap
src/parse.rs        text -> EnvFile; a Cursor parser, grammar in the //! doc
src/generator.rs    Generator: precedence, overrides, ${...} and $(...) resolution
src/render.rs       EnvFile + resolved values -> output text, output quoting
src/rand.rs         Seed and the derivation behind $(rand n)
src/main.rs         fn main only: runs cli::run, prints errors, exit code
src/cli.rs          clap top level, one variant per subcommand, dispatch
src/cli/generate.rs `envik generate`: arguments, reading inputs, writing output
tests/generate.rs   integration tests that run the binary
```

- **Imports point down only:** `env` at the bottom; `parse`, `render` and
  `rand` use only `env`; `generator` sits on top of them. Nothing in the
  library imports `cli`.
- **The library never touches the disk or the process environment.** The
  CLI reads files, environment variables and the seed, and hands values to
  the `Generator`. Library error messages never mention CLI flags; the CLI
  adds hints when it prints them.
- **A new subcommand** is a file in `src/cli/`, a variant in `cli::Command`
  and in `cli::Error`, and one arm in `cli::run`. Code used by one command
  lives in that command's file; move it out only when a second command needs
  it.

## Code style

These are the rules this codebase was reviewed against. Follow them in new
code; when old code breaks one, say so rather than refactoring in passing.

- **Unhappy path leaves early** with `let … else`, `?` or an early `return`.
  Never the inverse: no `let Err(e) = run() else { return Ok(()) };`
  followed by the error handling. Success is the last line.
- **One decision per `match`.** No `match` inside a `match` arm and no
  `match` on `Option<Result<…>>`: name the inner decision as a function, or
  use `let … else` steps, or a let chain (`if let Some(a) = x && let Some(b)
  = a.y`). Destructure one level per pattern.
- **Match arms share a shape.** If one arm needs a block, every arm gets
  braces.
- **Conditions ask, never act.** No side effects in a guard or an `&&`
  operand (`Some('$') if self.eat('{')`). No side effects in closures passed
  to `map`/`and_then`/`then`. No `bool::then` in place of an `if`.
- **`?` takes a value out; a check is an `if`** on a predicate.
- **A list of calls is written as calls**, not a `for` over an array of
  unrelated values. Two plain loops beat a helper invented to share five
  lines.
- **A case that cannot happen stays out of the signature** rather than being
  handled.
- **Types prove their invariants:** a constructor returns a valid value
  (`EnvKey::new`, `Seed::new`, `Generator::new`); nothing below re-checks.
  Keep `pub` as narrow as the callers need.
- **Errors:** `thiserror` everywhere; one enum per module that fails; every
  error a person fixes is collected and returned together as `Vec<E>`
  (`Generator::new -> Vec<ParseError>`, `generate -> Vec<GeneratorError>`),
  never just the first. Messages lowercase, no trailing period, and no two
  variants with the same text. `expect` only for a broken invariant, with a
  message that names it.
- **The parser** is one `Cursor` over a line with one position, one method
  per grammar rule, the grammar written in `parse.rs`'s `//!` doc. A rule
  that does not fit is a reason to change the grammar and the shape, not to
  add a special case.
- **Docs say why**, one sentence per decision; internal detail goes under
  `# Implementation`. Once one item in a struct, enum or method family has a
  doc, all of them get one.

## Behaviour that must not change by accident

Each of these has a test; changing one is a decision, recorded in
[docs/decisions.md](docs/decisions.md), and usually a breaking change.

- **Precedence:** input files in command-line order (`-i` and `-I`
  interleaved), then `-e`, then `--env-override-prefix`. Overrides replace
  only keys that exist (set, or offered as `# KEY=` by the template).
- **Layout:** the first file read is the template; an unchanged literal line
  comes out byte for byte; only the value span is replaced; keys the
  template lacks go at the end after a blank line and `# ----`; the output
  always ends with a newline.
- **Values from `-e` and overrides are literal**: `${...}` and `$(...)` never
  expand in them, nor inside `'...'`.
- **Output quoting:** bare when every character is in `[A-Za-z0-9_-./:@%+,=?]`,
  single quotes otherwise, double quotes with escapes only for `'` or a
  newline. Writing then reading must give the same value
  (`render::tests::written_values_read_back`).
- **`$(rand n)` values are pinned.** `rand::CONTEXT`, the field encoding in
  `Seed::rand` and the alphabet must never change: doing so rotates every
  user's secrets. `rand::tests::stable_per_key` pins one value.
- **The seed file** (`--seed-file-create`) is created only after a
  successful generation, mode 0600, never overwriting an existing one.
- **`-o` is written in place** (`fs::write`), so permissions, owner and
  symlinks are kept; nothing is written when anything failed.

## Tests

- Unit tests live next to the code (`#[cfg(test)] mod tests`); behaviour
  through the binary goes in `tests/generate.rs`.
- Integration tests set the child's environment with `Command::env` /
  `env_remove`, never `std::env::set_var`. Each test works in its own
  `scratch(...)` directory.
- A test exists to catch a specific bug: name it after the behaviour and make
  sure it fails when that behaviour breaks.

## Commits and releases

- **Conventional Commits** (`feat(generate): …`, `fix(release): …`,
  `refactor(cli): …`, `docs: …`), description capitalised. The changelog is
  generated from them by git-cliff (`cliff.toml`), and `feat`/`fix` decide
  the next version, so a change a user would notice is never a `chore`.
- One topic per branch and per PR, branched from an up-to-date `main`.
- **Releasing** is the "Prepare release" workflow (Actions → Run workflow on
  `main`, optional version). It runs `cargo release` (`release.toml`), which
  bumps the version, writes `CHANGELOG.md` with git-cliff, commits, tags and
  pushes with the deploy key; the tag starts the cargo-dist "Release"
  workflow. Before 1.0, `feat` and breaking changes bump the minor.
- `.github/workflows/release.yml` is generated by cargo-dist: never edit it.
  Change `dist-workspace.toml` and run `dist generate` (`dist generate
  --check` must pass).
- The npm package is `@envik/cli` (the name `envik` was refused as too close
  to `envify`) and uses npm staged publishing: each release waits for a
  maintainer's 2FA approval on npmjs.com.
- Linux binaries are static musl only, so they run on any distro, NixOS
  without nix-ld included.
