# Decisions

Choices that look odd, or that someone tried to undo, with the reason that
settled them. Reopening one needs a new reason, not the old argument again.

## Syntax and behaviour

- **Own parser, not a dotenv crate.** The template's layout, comments and
  inline comments must come out byte for byte, and only the value span be
  replaced; dotenv crates parse to key/value pairs and lose all of that.
  A parser crate (winnow, pest) was weighed against the hand-written cursor:
  the grammar is small, and the crates made the error messages, the
  `#`-after-a-blank rule and the `rand` bounds harder, not easier.
- **Calls are `$(rand n)`, not `${rand(n)}`.** Shell-like, and a call can no
  longer be confused with a reference: after `$`, `{` is a reference and `(`
  a call. `{{rand 32}}` was rejected: `{{` is common literal text in values
  (templates, JSON).
- **`$(required)` marks a value that must come from a later source.** It
  is a call rather than the shell's `${VAR:?message}`, because
  `KEY=${KEY:?…}` would be a key referencing itself, a cycle here. It takes
  no message: the error shows the whole template value, so text around it
  (`$(required) get one at …`) explains itself.
- **`${NAME:-default}` takes the default when `NAME` is unset or empty**,
  like the shell and docker compose. Defaults are literal; a nested `${...}`
  or `$(...)` in a default is an error. A `#` inside `${...}` is text.
- **`-e` values and env overrides are literal.** They come from a shell
  already; expanding them again would surprise.
- **Env overrides replace only existing keys**, so a stray variable in a CI
  environment never adds a key to the `.env`. They win over `-e`. A
  prefixed variable whose rest is not a valid key is skipped; a non-UTF-8
  value is an error only for a key that is used.
- **Commented `# KEY=` lines are optional variables:** turned on in place when
  something sets the key, dropped when nothing does. Only `#KEY=` and
  `# KEY=` at column 0 count; anything looser is prose.
- **Keys the template lacks go after a blank line and `# ----`**, with no
  text, since `.env` files are not always in English.
- **`?` is a bare output character** (a shell does not expand it in an
  assignment, so URLs with a query string stay unquoted); `&` and `~` are not,
  because a shell sourcing the file would act on them.
- **The output always ends with a newline**, whatever the template does.
- **`-I` keeps its place among the `-i`.** clap's derive loses the relative
  order of two arguments, so `cli/generate.rs` reads the argument positions
  from `ArgMatches`. Unknown argument ids panic in release builds too
  (`try_contains_id`): without that check, a renamed argument silently
  dropped every input file in release builds.

## Secrets

- **BLAKE3** (`derive_key` + keyed hash + XOF) covers key derivation, keyed
  hashing and variable-length output in one crate; HMAC-SHA256 would need
  HKDF and a counter loop on top.
- **`rand::CONTEXT` and the derivation are frozen.** Changing either rotates
  every user's secrets; `rand::tests::stable_per_key` pins a value.
- **`byte & 63` is unbiased** because the alphabet has exactly 64 characters;
  the two dropped bits per byte cost nothing (the XOF has no end), and each
  character still carries the 6 bits base64url can hold.
- **The seed file is created only after a successful generation.** Creating
  it as soon as the seed is made up was simpler, and was rejected to keep a
  failed run from leaving files behind.

## Library and CLI

- **`Generator` instead of a `generate(inputs, overrides, seed)` function.**
  It owns the precedence rule, so callers add files, flags, overrides and the
  seed in any order. `Generator::new` validates the files, so a `Generator`
  never holds an invalid one, and `generate` cannot return parse errors.
- **Errors are `Vec<E>`, one enum per step**: `Vec<ParseError>` from `new`,
  `Vec<GeneratorError>` from `generate`. A wrapper type around the list, or
  separate enums per internal step, added a type nobody matched on.
- **Merging and interpolation are one step inside the `Generator`**, not two
  public modules: from outside, "who wins" and "expand references" are one
  question, the final value of each key.
- **`cli/` is one file per subcommand**, with a command's helpers in its own
  file; only code two commands share moves out.
- **`-o` is written with `fs::write`, not atomically.** A temp file plus
  rename replaced a symlinked `.env` with a regular file and could change its
  owner; the half-written-file risk it guarded against is negligible for a
  file of a few KB that can be regenerated.

## Release

- **cargo-dist, triggered by a tag**, with a manual "Prepare release"
  workflow running `cargo release` and git-cliff. The tag is pushed with a
  deploy key because a tag pushed with `GITHUB_TOKEN` does not trigger other
  workflows. Both release workflows show up in Actions; that is the price of
  never editing the generated `release.yml`.
- **npm package `@envik/cli`.** `envik` was refused by the registry as too
  similar to `envify`. npm staged publishing makes every release wait for a
  2FA approval; trusted publishing (no `NPM_TOKEN`) is possible later through
  a custom publish job.
- **Linux is static musl only (x86_64, aarch64).** The installers picked the
  glibc build whenever `ldd` reported glibc, which broke NixOS without nix-ld
  and left Alpine on ARM with nothing; envik makes few allocations and no
  network calls, so musl's allocator and DNS do not matter.
- **Old releases stay.** Each npm version downloads its binary from its
  GitHub Release, so deleting a release breaks installing that version.
