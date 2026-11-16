# envik

Builds `.env` files from templates, overrides and command-line values.

## `envik generate`

```sh
envik generate -i .env.example -i .env.override -e PORT=445 -o .env
```

| Option | |
|---|---|
| `-i, --input <FILE>` | Input file. Repeatable, at least one. |
| `-e, --env <KEY=VALUE>` | A value, taken literally. Repeatable. |
| `-o, --output <FILE>` | Output file, created or overwritten. Without it, the result goes to stdout. |

**Precedence.** The `-i` files apply in the order given, later ones winning.
Every `-e` applies after all files and always wins, wherever it sits on the
command line.

**Layout.** The first `-i` is the template: its comments, blank lines and key
order come out unchanged, and each `KEY=value` gets its final value. Only the
value is replaced; spacing and an inline comment (`PORT=3000  # api`) stay.
Keys that the template does not have are appended at the end, in the order
they first appeared. Comments from the other files are dropped.

**Errors.** A missing input, an invalid line or an invalid `-e` prints an
error and exits non-zero. Nothing is written: an existing output file stays
as it was.

## Syntax

```sh
# comment
KEY=value
KEY = value   # inline comment (`#` after a blank)
EMPTY=
```

Variable names match `[A-Za-z_][A-Za-z0-9_]*`. Spaces around `=` and around
the value are trimmed.
