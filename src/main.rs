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

/// `--seed-env` value parser: a name `std::env::var_os` accepts.
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
    let (name, value) = match arg.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (arg, None),
    };
    let key = Key::new(name).ok_or_else(|| format!("invalid variable name `{name}`"))?;
    Ok(match value {
        Some(value) => Override::Literal(key, value.to_owned()),
        None => Override::FromEnv(key),
    })
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
    let out = envik::generate(&sources, &inputs.overrides, inputs.seed.as_ref())?;
    // Before the output: a written .env must never rely on a seed that was
    // not saved.
    if let Some((path, text)) = &inputs.seed_to_create {
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
    seed: Option<Seed>,
    /// A `--seed-file` that did not exist, with the seed made for it: written
    /// only once the generation has succeeded.
    seed_to_create: Option<(PathBuf, String)>,
}

/// Reads every `-i` file and looks up every `-e KEY`; on failure, returns
/// every reason an input could not be read.
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
        match item {
            Override::Literal(key, value) => overrides.push((key.clone(), value.clone())),
            Override::FromEnv(key) => match env::var_os(key.as_str()) {
                None => errors.push(InputError::EnvMissing(key.clone())),
                Some(value) => match value.into_string() {
                    Ok(value) => overrides.push((key.clone(), value)),
                    Err(_) => errors.push(InputError::EnvNotUtf8(key.clone())),
                },
            },
        }
    }
    let (seed_text, seed_to_create) = read_seed(args, &mut errors);
    let seed = seed_text.and_then(|text| {
        let seed = Seed::new(text.as_bytes());
        if seed.is_none() {
            errors.push(InputError::EmptySeed);
        }
        seed
    });
    if errors.is_empty() {
        Ok(Inputs {
            contents,
            overrides,
            seed,
            seed_to_create,
        })
    } else {
        Err(errors)
    }
}

/// The seed text from whichever of `--seed-value`, `--seed-env` or
/// `--seed-file` was given (at most one), and the file to create when
/// `--seed-file-create` had to make one up. Failures go to `errors`.
fn read_seed(
    args: &GenerateArgs,
    errors: &mut Vec<InputError>,
) -> (Option<String>, Option<(PathBuf, String)>) {
    if let Some(text) = &args.seed_value {
        return (Some(text.clone()), None);
    }
    if let Some(name) = &args.seed_env {
        let text = match env::var_os(name.as_str()).map(|value| value.into_string()) {
            None => {
                errors.push(InputError::EnvMissing(name.clone()));
                None
            }
            Some(Err(_)) => {
                errors.push(InputError::EnvNotUtf8(name.clone()));
                None
            }
            Some(Ok(text)) => Some(text),
        };
        return (text, None);
    }
    let Some(path) = &args.seed_file else {
        return (None, None);
    };
    match fs::read_to_string(path) {
        Ok(text) => {
            let text = text.strip_suffix('\n').unwrap_or(&text);
            let text = text.strip_suffix('\r').unwrap_or(text);
            (Some(text.to_owned()), None)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && args.seed_file_create => {
            match envik::new_seed_text() {
                Ok(text) => (Some(text.clone()), Some((path.clone(), text))),
                Err(error) => {
                    errors.push(InputError::Random(error));
                    (None, None)
                }
            }
        }
        Err(source) => {
            errors.push(InputError::Read {
                path: path.clone(),
                source,
            });
            (None, None)
        }
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
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(contents.as_bytes())?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
