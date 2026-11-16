//! Command line: argument parsing and all of the I/O. The merging itself is
//! [`envik::generate`], which never touches the disk.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use envik::{Key, Source};

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
}

/// `-e` value parser: checks the key here so a bad name is a usage error
/// reported by clap, before any file is read.
/// One `-e`. The environment is read later, with the input files, so every
/// missing variable is reported in one run.
#[derive(Clone, Debug)]
enum Override {
    Literal(Key, String),
    FromEnv(Key),
}

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
    let out = envik::generate(&sources, &inputs.overrides)?;
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
    if errors.is_empty() {
        Ok(Inputs {
            contents,
            overrides,
        })
    } else {
        Err(errors)
    }
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
