//! Command line: argument parsing and all of the I/O. The merging itself is
//! [`envik::generate`], which never touches the disk.

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

    /// KEY=VALUE taken literally; repeat for more. Always overrides the files.
    #[arg(short, long = "env", value_name = "KEY=VALUE", value_parser = parse_override)]
    env: Vec<(Key, String)>,

    /// Output file, created or overwritten. Prints to stdout when omitted.
    #[arg(short, long = "output", value_name = "FILE")]
    output: Option<PathBuf>,
}

/// `-e` value parser: checks the key here so a bad name is a usage error
/// reported by clap, before any file is read.
fn parse_override(arg: &str) -> Result<(Key, String), String> {
    let (name, value) = arg
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE, found `{arg}`"))?;
    let key = Key::new(name).ok_or_else(|| format!("invalid variable name `{name}`"))?;
    Ok((key, value.to_owned()))
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
    /// Every input that could not be read (never empty).
    #[error("{}", one_per_line(.0))]
    Read(Vec<ReadError>),
    #[error("{}: {source}", path.display())]
    Write { path: PathBuf, source: io::Error },
    #[error("stdout: {0}")]
    Stdout(io::Error),
    #[error(transparent)]
    Generate(#[from] envik::Error),
}

#[derive(Debug, thiserror::Error)]
#[error("{}: {source}", path.display())]
struct ReadError {
    path: PathBuf,
    source: io::Error,
}

fn one_per_line(errors: &[ReadError]) -> String {
    errors
        .iter()
        .map(ReadError::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn generate(args: &GenerateArgs) -> Result<(), CliError> {
    let mut contents = Vec::with_capacity(args.input.len());
    let mut errors = Vec::new();
    for path in &args.input {
        match fs::read_to_string(path) {
            Ok(text) => contents.push(text),
            Err(source) => errors.push(ReadError {
                path: path.clone(),
                source,
            }),
        }
    }
    if !errors.is_empty() {
        return Err(CliError::Read(errors));
    }
    let sources: Vec<Source<'_>> = args
        .input
        .iter()
        .zip(&contents)
        .map(|(origin, contents)| Source { origin, contents })
        .collect();
    let out = envik::generate(&sources, &args.env)?;
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
