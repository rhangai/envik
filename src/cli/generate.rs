//! `envik generate`: reads the files, environment variables and seed the
//! arguments point at, hands them to [`envik::Generator`], and writes the
//! result to `-o` or stdout.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{ArgGroup, ArgMatches, Args};

use envik::{EnvKey, Generator, GeneratorError, ParseError, Seed, Source};

use crate::util::write::create_private;

#[derive(Args)]
#[command(group(ArgGroup::new("seed").multiple(false)))]
pub struct GenerateArgs {
    /// Input file; repeat for more. Later files override earlier ones. The
    /// first one read is the layout template (comments, order, blank lines).
    #[arg(short, long = "input", value_name = "FILE", required = true)]
    input: Vec<PathBuf>,

    /// Like -i, but skipped when the file does not exist. Takes its place
    /// among the -i in the order given.
    #[arg(short = 'I', long = "input-optional", value_name = "FILE")]
    input_optional: Vec<PathBuf>,

    /// KEY=VALUE, or KEY to take its value from the environment; repeat for
    /// more. Always overrides the files, and is taken literally.
    #[arg(short, long = "env", value_name = "KEY[=VALUE]", value_parser = parse_override)]
    env: Vec<Override>,

    /// Output file, created or overwritten. Prints to stdout when omitted.
    #[arg(short, long = "output", value_name = "FILE")]
    output: Option<PathBuf>,

    /// Every environment variable PREFIX + KEY replaces KEY, when the inputs
    /// already have KEY (even as `# KEY=`). Wins over the files and -e, and
    /// is taken literally. E.g. CI_OVERRIDE_ turns CI_OVERRIDE_DB_PASS into
    /// DB_PASS.
    #[arg(long, value_name = "PREFIX", value_parser = parse_prefix)]
    env_override_prefix: Option<String>,

    /// Seed for $(rand n), given here. It stays in your shell history: use
    /// it locally.
    #[arg(long, value_name = "TEXT", group = "seed", allow_hyphen_values = true)]
    seed_value: Option<String>,

    /// Seed for $(rand n), read from FILE. Keep the file out of git.
    #[arg(long, value_name = "FILE", group = "seed")]
    seed_file: Option<PathBuf>,

    /// Create the --seed-file with a random seed if it does not exist yet
    /// (only when the generation succeeds).
    #[arg(long, requires = "seed_file")]
    seed_file_create: bool,

    /// Seed for $(rand n), read from the environment variable VAR.
    #[arg(long, value_name = "VAR", group = "seed", value_parser = parse_env_name)]
    seed_env: Option<EnvKey>,
}

/// `--env-override-prefix` value parser: an empty prefix would take the
/// whole environment.
fn parse_prefix(arg: &str) -> Result<String, String> {
    if arg.is_empty() {
        return Err("the prefix cannot be empty".to_owned());
    }
    Ok(arg.to_owned())
}

/// `--seed-env` value parser, and the key of every `-e`.
fn parse_env_name(arg: &str) -> Result<EnvKey, String> {
    EnvKey::new(arg).ok_or_else(|| format!("invalid variable name `{arg}`"))
}

/// One `-e`. The environment is read later, with the input files, so every
/// missing variable is reported in one run.
#[derive(Clone, Debug)]
enum Override {
    Literal(EnvKey, String),
    FromEnv(EnvKey),
}

/// `-e` value parser: checks the key here so a bad name is a usage error
/// reported by clap, before any file is read.
fn parse_override(arg: &str) -> Result<Override, String> {
    match arg.split_once('=') {
        Some((name, value)) => Ok(Override::Literal(parse_env_name(name)?, value.to_owned())),
        None => Ok(Override::FromEnv(parse_env_name(arg)?)),
    }
}

/// What can go wrong running `envik generate`, around the library: the I/O
/// it does not do.
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    /// Every input file or `-e KEY` that could not be read (never empty);
    /// see [`read_inputs`].
    #[error("{}", one_per_line(.0))]
    Input(Vec<InputError>),
    /// Every invalid line, across all inputs.
    #[error("{}", one_per_line(.0))]
    Parse(Vec<ParseError>),
    /// The `-o` file, or the seed file being created.
    #[error("{}: {source}", path.display())]
    Write { path: PathBuf, source: io::Error },
    /// The output, without `-o`.
    #[error("stdout: {0}")]
    Stdout(io::Error),
    /// Every reason the generation could not finish.
    #[error("{}", one_per_line(.0))]
    Generate(Vec<GeneratorError>),
}

/// Something the arguments point at — a file, an environment variable or
/// the seed — that could not be read. Reported all together.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("{}: {source}", path.display())]
    FileRead { path: PathBuf, source: io::Error },
    #[error("`{0}` is not set in the environment")]
    EnvMissing(EnvKey),
    #[error("`{0}` in the environment is not valid UTF-8")]
    EnvNotUtf8(EnvKey),
    #[error("the seed is empty")]
    EmptySeed,
    #[error("could not create a seed: {0}")]
    Random(getrandom::Error),
}

fn one_per_line<T: std::fmt::Display>(errors: &[T]) -> String {
    errors
        .iter()
        .map(T::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// One `-i` or `-I`.
struct InputFile {
    /// As given on the command line.
    path: PathBuf,
    /// `-I`: skipped when the file does not exist.
    optional: bool,
    /// Where it sat on the command line, to put `-i` and `-I` back in order.
    position: usize,
}

/// Every `-i` and `-I`, in the order they were given.
fn input_files(args: &GenerateArgs, matches: &ArgMatches) -> Vec<InputFile> {
    let mut files = Vec::with_capacity(args.input.len() + args.input_optional.len());
    let positions = matches.indices_of("input").into_iter().flatten();
    for (path, position) in args.input.iter().zip(positions) {
        files.push(InputFile {
            path: path.clone(),
            optional: false,
            position,
        });
    }
    let positions = matches.indices_of("input_optional").into_iter().flatten();
    for (path, position) in args.input_optional.iter().zip(positions) {
        files.push(InputFile {
            path: path.clone(),
            optional: true,
            position,
        });
    }
    files.sort_by_key(|file| file.position);
    files
}

/// Runs `envik generate`. `matches` are its raw matches, for where each `-i`
/// and `-I` sat on the command line.
pub fn run(args: &GenerateArgs, matches: &ArgMatches) -> Result<(), GenerateError> {
    let files = input_files(args, matches);
    let inputs = read_inputs(args, &files).map_err(GenerateError::Input)?;
    let sources: Vec<Source<'_>> = inputs
        .contents
        .iter()
        .map(|(origin, contents)| Source { origin, contents })
        .collect();
    let mut generator = Generator::new(&sources).map_err(GenerateError::Parse)?;
    for (key, value) in inputs.env_flags {
        generator.env_flag(key, value);
    }
    for (key, value) in inputs.env_overrides {
        generator.env_override(key, value);
    }
    if let Some(seed) = inputs.seed {
        generator.seed(seed);
    }
    let out = generator.generate().map_err(GenerateError::Generate)?;
    // Before the output: a written .env must never rely on a seed that was
    // not saved.
    if let Some(NewSeedFile { path, text }) = &inputs.new_seed_file {
        // The newline makes it a text file; reading it back strips it.
        create_private(path, &format!("{text}\n")).map_err(|source| GenerateError::Write {
            path: path.clone(),
            source,
        })?;
    }
    match &args.output {
        // Over the existing file, not a renamed copy: its permissions, owner
        // and any symlink to it stay as they were.
        Some(path) => fs::write(path, &out).map_err(|source| GenerateError::Write {
            path: path.clone(),
            source,
        })?,
        None => io::stdout()
            .lock()
            .write_all(out.as_bytes())
            .map_err(GenerateError::Stdout)?,
    }
    Ok(())
}

/// Everything the library is handed, read from disk and the environment.
struct Inputs {
    /// Every `-i` and every `-I` that exists, in the order given, with its
    /// path.
    contents: Vec<(PathBuf, String)>,
    /// One per `-e`, in the same order, `-e KEY` already looked up.
    env_flags: Vec<(EnvKey, String)>,
    /// From `--env-override-prefix`, sorted by key; values not yet checked
    /// for UTF-8, since only the ones used must be.
    env_overrides: Vec<(EnvKey, OsString)>,
    /// From `--seed-value`, `--seed-env` or `--seed-file`, when one was given.
    seed: Option<Seed>,
    /// Written only after the generation succeeds.
    new_seed_file: Option<NewSeedFile>,
}

/// Reads every `-i` file and every `-I` that exists, looks up every
/// `-e KEY` and reads the seed; on failure, returns every reason an input
/// could not be read.
fn read_inputs(args: &GenerateArgs, files: &[InputFile]) -> Result<Inputs, Vec<InputError>> {
    let mut errors = Vec::new();
    let mut contents = Vec::with_capacity(files.len());
    for file in files {
        match fs::read_to_string(&file.path) {
            Ok(text) => {
                contents.push((file.path.clone(), text));
            }
            // Only a missing file is optional: one that exists but cannot be
            // read is still an error.
            Err(error) if file.optional && error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                errors.push(InputError::FileRead {
                    path: file.path.clone(),
                    source,
                });
            }
        }
    }
    let mut env_flags = Vec::with_capacity(args.env.len());
    for item in &args.env {
        match read_override(item) {
            Ok(value) => env_flags.push(value),
            Err(error) => errors.push(error),
        }
    }
    let env_overrides = match &args.env_override_prefix {
        Some(prefix) => read_env_overrides(prefix),
        None => Vec::new(),
    };
    let mut seed = None;
    let mut new_seed_file = None;
    match read_seed(args) {
        Ok(None) => {}
        Ok(Some(input)) => {
            seed = Some(input.seed);
            new_seed_file = input.new_file;
        }
        Err(error) => {
            errors.push(error);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Inputs {
        contents,
        env_flags,
        env_overrides,
        seed,
        new_seed_file,
    })
}

/// One `-e`, with `-e KEY` looked up in the environment.
fn read_override(item: &Override) -> Result<(EnvKey, String), InputError> {
    match item {
        Override::Literal(key, value) => Ok((key.clone(), value.clone())),
        Override::FromEnv(key) => Ok((key.clone(), read_env(key)?)),
    }
}

/// Every environment variable named `prefix` + a valid key, as that key,
/// sorted so the output and errors are the same on every run. Any other
/// variable, including a prefixed one whose rest is not a key, is skipped:
/// it could never replace anything.
fn read_env_overrides(prefix: &str) -> Vec<(EnvKey, OsString)> {
    let mut overrides = Vec::new();
    for (name, value) in env::vars_os() {
        // Bytes, so a non-UTF-8 name elsewhere in the environment is no error.
        let Some(rest) = name.as_encoded_bytes().strip_prefix(prefix.as_bytes()) else {
            continue;
        };
        let Ok(rest) = std::str::from_utf8(rest) else {
            continue;
        };
        let Some(key) = EnvKey::new(rest) else {
            continue;
        };
        overrides.push((key, value));
    }
    overrides.sort_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()));
    overrides
}

/// `key` from the process environment.
fn read_env(key: &EnvKey) -> Result<String, InputError> {
    let Some(value) = env::var_os(key.as_str()) else {
        return Err(InputError::EnvMissing(key.clone()));
    };
    value
        .into_string()
        .map_err(|_| InputError::EnvNotUtf8(key.clone()))
}

/// The seed, and the file to create for it when it was just made up.
struct SeedInput {
    seed: Seed,
    new_file: Option<NewSeedFile>,
}

/// A `--seed-file` that did not exist, with the seed text made for it:
/// written only once the generation has succeeded.
struct NewSeedFile {
    path: PathBuf,
    text: String,
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
    let new_file = create_at.map(|path| NewSeedFile { path, text });
    Ok(Some(SeedInput { seed, new_file }))
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
        Err(source) => Err(InputError::FileRead {
            path: path.to_owned(),
            source,
        }),
    }
}
