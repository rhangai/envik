# Changelog

All notable changes to this project are documented here.

## [0.1.1] - 2026-11-17

### Fixes

- **release:** Publish the npm package as @envik/cli

## [0.1.0] - 2026-11-17

### Features

- **generate:** Merge env files and values into a .env
- **generate:** Parse quoted values and escapes
- **generate:** Turn on or drop commented variables in the template
- **generate:** Expand ${VAR} references and quote output minimally
- **generate:** Read -e KEY values from the environment
- **generate:** Add ${rand(n)} secrets derived from a seed
- **generate:** Write calls as \$(rand n) and parse references with the cursor

### Fixes

- **generate:** Report a missing seed alongside other errors in the same value

### Refactor

- **generate:** Flatten nested matches and clever combinators
- **generate:** Split the .env model from parsing and flatten control flow
- **generate:** Split the .env model from parsing and flatten control flow
- **generate:** Split the .env model from parsing and rewrite the parser

### CI

- Check format, lint and tests on pull requests and main
- Build releases with cargo-dist and publish to npm
- Prepare releases with cargo-release and git-cliff from a workflow

### Others

- Initial commit

