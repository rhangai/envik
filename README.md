# envik

Builds `.env` files from templates, overrides and command-line values.

## Install

With npm:

```sh
npm i -g @envik/cli
# or, without installing
npx @envik/cli generate -i .env.example -o .env
```

With the installer script (macOS and Linux):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/rhangai/envik/releases/latest/download/envik-installer.sh | sh
```

On Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/rhangai/envik/releases/latest/download/envik-installer.ps1 | iex"
```

Or download a binary from the [releases page](https://github.com/rhangai/envik/releases):
Linux (x86_64, ARM64, and a static musl build), macOS (Intel and Apple
Silicon) and Windows (x86_64).

## `envik generate`

```sh
envik generate -i .env.example -i .env.override -e PORT=445 -o .env
```

| Option | |
|---|---|
| `-i, --input <FILE>` | Input file. Repeatable, at least one. |
| `-e, --env <KEY[=VALUE]>` | A value, taken literally. With just `KEY`, the value comes from your environment (`-e PORT`). Repeatable. |
| `-o, --output <FILE>` | Output file, created or overwritten. Without it, the result goes to stdout. |

**Precedence.** The `-i` files apply in the order given, later ones winning.
Every `-e` applies after all files and always wins, wherever it sits on the
command line.

**Layout.** The first `-i` is the template: its comments, blank lines and key
order come out unchanged, and each `KEY=value` gets its final value. Only the
value is replaced; spacing and an inline comment (`PORT=3000  # api`) stay.
Keys that the template does not have are appended at the end, in the order
they first appeared, after a blank line and a `# ----`
line. Comments from the other files are dropped.

```sh
DB_URL=postgres://localhost/app

# ----
EXTRA=2
```

**Optional variables.** A template line that is exactly `#KEY=...` or
`# KEY=...` is an optional variable:

```sh
# Turn on verbose logs
# DEBUG=false  # any value
```

- If some file or `-e` sets `DEBUG`, the line is turned on in place:
  `DEBUG=1  # any value`. Only the first such line per key is.
- If nothing sets `DEBUG`, the line is left out of the output.
- If the template also has an active `DEBUG=` line, the commented one stays
  as it is.

Anything looser is plain text and never touched: `# DEBUG` (no `=`),
`#  DEBUG=` (two spaces), an indented `  # DEBUG=`.

**Errors.** A missing input, an invalid line, an invalid `-e` or an `-e KEY`
that is not in your environment prints an error and exits non-zero. An empty
variable (`PORT=`) is a value, not an error. Nothing is written: an existing output file stays
as it was.

## Syntax

```sh
# comment
KEY=value
KEY = value   # inline comment (`#` after a blank)
EMPTY=
QUOTED="line one\nline two"
LITERAL='nothing in here is special: \n ${X}'
```

Variable names match `[A-Za-z_][A-Za-z0-9_]*`. Spaces around `=` and around
an unquoted value are trimmed.

| Form | Escapes | `${...}` |
|---|---|---|
| unquoted | none | yes |
| `"..."` | `\n`, `\"`, `\\`, `\$`; any other is an error | yes |
| `'...'` | none | no |

After a closing quote, only blanks or a blank and a `#` comment may follow.
Anything else — `export KEY=...`, an unclosed quote, an unknown escape — is an
error that names the file and line. Every invalid line is reported in one run.

## References

```sh
URL=https://${BASE}
PORT=123
BASE=site:${PORT}
LOG=${LOG_LEVEL:-info}
```

gives `URL=https://site:123`, `BASE=site:123` and `LOG=info`.

- `${NAME}` takes `NAME`'s final value, after every file and `-e` is applied,
  whatever order the lines are in. With `-e PORT=445` above, both `BASE` and
  `URL` change.
- `${NAME:-default}` takes `default` when `NAME` is unset or empty. The
  default is plain text and cannot hold another `${...}` or a `$(...)`. A
  `#` inside `${...}` is part of it, not a comment.
- A reference to something unset, with no default, is an error; so is a loop
  (`A=${B}`, `B=${A}`). Every one is reported in the same run, and nothing is
  written.
- Nothing expands inside `'...'`, `$NAME` without braces stays as it is, and
  values given with `-e` are always literal.

## Generated secrets

```sh
APP_SECRET=$(rand 32)
```

`$(rand n)` writes `n` characters (1 to 1024) from `A-Z a-z 0-9 - _`. They
look random, but they come from a seed, so running `generate` again gives the
same secrets and nothing gets rotated by accident. The value depends only on
the seed, the variable's name and which `rand` it is inside that value:
adding or moving other lines never changes it. Two variables never get the
same value, and `OTHER=${APP_SECRET}` copies it.

Give the seed one way, at most:

| Option | |
|---|---|
| `--seed-value <TEXT>` | The seed itself. It stays in your shell history, so use it locally. |
| `--seed-file <FILE>` | Read it from a file. Add the file to `.gitignore`. |
| `--seed-file-create` | With `--seed-file`: create the file with a random seed if it is not there yet. Readable by you only, and written only when the generation succeeds. |
| `--seed-env <VAR>` | Read it from an environment variable, e.g. a CI secret. |

```sh
echo .env-seed >> .gitignore
envik generate -i .env.example -o .env --seed-file .env-seed --seed-file-create
```

It's written like a shell command: `$(rand 32)`, spaces allowed around the
words. A `rand` with no seed, an empty seed, `$(rand 0)`, `$(rand)` or an
unknown function is an error. Like a reference, `$(...)` does nothing inside
`'...'` or in a `-e` value; a `$` followed by neither `{` nor `(` is just
text.

Anyone with the seed can regenerate your secrets: keep it as secret as they
are.

## Output

The output has every reference expanded. A template line that holds no
reference and keeps its value comes out exactly as it was. Any other value is
written:

- bare, when it only has letters, digits and `_-./:@%+,=`;
- in single quotes when it needs quoting: `'a b'`, `'$5'`;
- in double quotes with escapes when it holds a `'` or a newline:
  `"it's"`, `"a\nb"`.

Reading the output back gives the same values.

## Releasing

For maintainers: Actions → **Prepare release** → Run workflow, on `main`.
Give a version (`0.2.0`) or leave it empty to work it out from the commits
since the last tag (`feat` bumps the minor, `fix` the patch). It bumps
`Cargo.toml`, writes `CHANGELOG.md`, commits, tags and pushes; the tag starts
the **Release** workflow, which builds every target, creates the GitHub
Release and publishes to npm. Locally, `cargo release 0.2.0` shows what it
would do, without changing anything.

It needs two repository secrets: `RELEASE_DEPLOY_KEY`, the private half of a
deploy key with write access (a tag pushed with the default token would not
start the Release workflow), and `NPM_TOKEN`, an npm token that can publish.
