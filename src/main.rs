//! Command line: argument parsing and all of the I/O. The merging itself is
//! [`envik::generate`], which never touches the disk.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{ArgGroup, Args, Parser, Subcommand};

use envik::{Key, Seed, Source};

/// Builds `.env` files from templates, overrides and command-line values.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Merge env files and values into a single .env.
    Generate(GenerateArgs),
}

#[derive(Args)]
#[command(group(ArgGroup::new("seed").multiple(false)))]
struct GenerateArgs {
    /// Input file; repeat for more. Later files override earlier ones. The
    /// first one is the layout template (comments, order, blank lines).
    #[arg(short, long = "input", value_name = "FILE", required = true)]
    input: Vec<PathBuf>,

    /// KEY=VALUE, or KEY to take its value from the environment; repeat for
    /// more. Always overrides the files, and is taken literally.
    #[arg(short, long = "env", value_name = "KEY[=VALUE]", value_parser = parse_override)]
    env: Vec<Override>,

    /// Output file, created or overwritten. Prints to stdout when omitted.
    #[arg(short, long = "output", value_name = "FILE")]
    output: Option<PathBuf>,

    /// Seed for ${rand(n)}, given here. It stays in your shell history: use
    /// it locally.
    #[arg(long, value_name = "TEXT", group = "seed")]
    seed_value: Option<String>,

    /// Seed for ${rand(n)}, read from FILE. Keep the file out of git.
    #[arg(long, value_name = "FILE", group = "seed")]
    seed_file: Option<PathBuf>,

    /// Create the --seed-file with a random seed if it does not exist yet
    /// (only when the generation succeeds).
    #[arg(long, requires = "seed_file")]
    seed_file_create: bool,

    /// Seed for ${rand(n)}, read from the environment variable VAR.
    #[arg(long, value_name = "VAR", group = "seed", value_parser = parse_env_name)]
    seed_env: Option<Key>,
}

/// `--seed-env` value parser, and the key of every `-e`.
fn parse_env_name(arg: &str) -> Result<Key, String> {
    Key::new(arg).ok_or_else(|| format!("invalid variable name `{arg}`"))
}

/// One `-e`. The environment is read later, with the input files, so every
/// missing variable is reported in one run.
#[derive(Clone, Debug)]
enum Override {
    Literal(Key, String),
    FromEnv(Key),
}

/// `-e` value parser: checks the key here so a bad name is a usage error
/// reported by clap, before any file is read.
fn parse_override(arg: &str) -> Result<Override, String> {
    match arg.split_once('=') {
        Some((name, value)) => Ok(Override::Literal(parse_env_name(name)?, value.to_owned())),
        None => Ok(Override::FromEnv(parse_env_name(arg)?)),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Generate(args) => generate(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            for line in error.to_string().lines() {
                eprintln!("envik: {line}");
            }
            ExitCode::FAILURE
        }
    }
}

/// What can go wrong around the library: the I/O it does not do.
#[derive(Debug, thiserror::Error)]
enum CliError {
    /// Every input file or `-e KEY` that could not be read (never empty);
    /// see [`read_inputs`].
    #[error("{}", one_per_line(.0))]
    Input(Vec<InputError>),
    #[error("{}: {source}", path.display())]
    Write { path: PathBuf, source: io::Error },
    #[error("stdout: {0}")]
    Stdout(io::Error),
    #[error(transparent)]
    Generate(#[from] envik::Error),
}

#[derive(Debug, thiserror::Error)]
enum InputError {
    #[error("{}: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("`{0}` is not set in the environment")]
    EnvMissing(Key),
    #[error("`{0}` in the environment is not valid UTF-8")]
    EnvNotUtf8(Key),
    #[error("the seed is empty")]
    EmptySeed,
    #[error("could not create a seed: {0}")]
    Random(getrandom::Error),
}

fn one_per_line(errors: &[InputError]) -> String {
    errors
        .iter()
        .map(InputError::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn generate(args: &GenerateArgs) -> Result<(), CliError> {
    let inputs = read_inputs(args).map_err(CliError::Input)?;
    let sources: Vec<Source<'_>> = args
        .input
        .iter()
        .zip(&inputs.contents)
        .map(|(origin, contents)| Source { origin, contents })
        .collect();
    let seed = inputs.seed.as_ref().map(|input| &input.seed);
    let out = envik::generate(&sources, &inputs.overrides, seed)?;
    // Before the output: a written .env must never rely on a seed that was
    // not saved.
    if let Some(SeedInput {
        text,
        create_at: Some(path),
        ..
    }) = &inputs.seed
    {
        create_seed_file(path, text).map_err(|source| CliError::Write {
            path: path.clone(),
            source,
        })?;
    }
    match &args.output {
        Some(path) => write_atomically(path, &out).map_err(|source| CliError::Write {
            path: path.clone(),
            source,
        })?,
        None => io::stdout()
            .lock()
            .write_all(out.as_bytes())
            .map_err(CliError::Stdout)?,
    }
    Ok(())
}

/// Everything the library is handed, read from disk and the environment.
struct Inputs {
    /// One per `-i`, in the same order.
    contents: Vec<String>,
    /// One per `-e`, in the same order, `-e KEY` already looked up.
    overrides: Vec<(Key, String)>,
    seed: Option<SeedInput>,
}

/// Reads every `-i` file, looks up every `-e KEY` and reads the seed; on
/// failure, returns every reason an input could not be read.
fn read_inputs(args: &GenerateArgs) -> Result<Inputs, Vec<InputError>> {
    let mut errors = Vec::new();
    let mut contents = Vec::with_capacity(args.input.len());
    for path in &args.input {
        match fs::read_to_string(path) {
            Ok(text) => contents.push(text),
            Err(source) => errors.push(InputError::Read {
                path: path.clone(),
                source,
            }),
        }
    }
    let mut overrides = Vec::with_capacity(args.env.len());
    for item in &args.env {
        match read_override(item) {
            Ok(value) => overrides.push(value),
            Err(error) => errors.push(error),
        }
    }
    let seed = match read_seed(args) {
        Ok(seed) => seed,
        Err(error) => {
            errors.push(error);
            None
        }
    };
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Inputs {
        contents,
        overrides,
        seed,
    })
}

/// One `-e`, with `-e KEY` looked up in the environment.
fn read_override(item: &Override) -> Result<(Key, String), InputError> {
    match item {
        Override::Literal(key, value) => Ok((key.clone(), value.clone())),
        Override::FromEnv(key) => Ok((key.clone(), read_env(key)?)),
    }
}

/// `key` from the process environment.
fn read_env(key: &Key) -> Result<String, InputError> {
    let Some(value) = env::var_os(key.as_str()) else {
        return Err(InputError::EnvMissing(key.clone()));
    };
    value
        .into_string()
        .map_err(|_| InputError::EnvNotUtf8(key.clone()))
}

/// The seed, and where it came from.
struct SeedInput {
    seed: Seed,
    /// What `seed` was made from, to write to `create_at`.
    text: String,
    /// A `--seed-file` that did not exist: written with `text` only once the
    /// generation has succeeded.
    create_at: Option<PathBuf>,
}

/// The seed from whichever of `--seed-value`, `--seed-env` or `--seed-file`
/// was given (at most one); `None` when none was.
fn read_seed(args: &GenerateArgs) -> Result<Option<SeedInput>, InputError> {
    let (text, create_at) = if let Some(text) = &args.seed_value {
        (text.clone(), None)
    } else if let Some(name) = &args.seed_env {
        (read_env(name)?, None)
    } else if let Some(path) = &args.seed_file {
        read_seed_file(path, args.seed_file_create)?
    } else {
        return Ok(None);
    };
    let Some(seed) = Seed::new(text.as_bytes()) else {
        return Err(InputError::EmptySeed);
    };
    Ok(Some(SeedInput {
        seed,
        text,
        create_at,
    }))
}

/// `path`'s seed; or, when it does not exist and `create` is set, a new one
/// to write there once the generation succeeds.
fn read_seed_file(path: &Path, create: bool) -> Result<(String, Option<PathBuf>), InputError> {
    match fs::read_to_string(path) {
        Ok(text) => {
            // The newline an editor or `echo` adds is not part of the seed.
            let text = text.strip_suffix('\n').unwrap_or(&text);
            let text = text.strip_suffix('\r').unwrap_or(text);
            Ok((text.to_owned(), None))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
            let text = envik::new_seed_text().map_err(InputError::Random)?;
            Ok((text, Some(path.to_owned())))
        }
        Err(source) => Err(InputError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

/// Creates the seed file readable by its owner only, failing if it appeared
/// in the meantime rather than replacing someone else's seed.
fn create_seed_file(path: &Path, text: &str) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.write_all(b"\n")
}

/// Writes to a sibling temporary file, then renames it over `path`, so a
/// failure never leaves `path` half-written. An existing `path` keeps its
/// permissions; a new one gets the system default.
///
/// # Implementation
///
/// The permissions go on the temporary file before the contents do, so a
/// `0600` file holding secrets is never readable more widely, even briefly.
fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("not a file path"))?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".envik-{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let permissions = match fs::metadata(path) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Err(error) = write_then_rename(&tmp, path, permissions, contents) {
        // Leave no temporary file behind; the write error is what matters.
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

/// Writes `contents` to `tmp` with `permissions`, then moves it over `path`.
fn write_then_rename(
    tmp: &Path,
    path: &Path,
    permissions: Option<fs::Permissions>,
    contents: &str,
) -> io::Result<()> {
    let mut file = fs::File::create(tmp)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.write_all(contents.as_bytes())?;
    fs::rename(tmp, path)
}
