//! The `envik` command line: the arguments, and one module per subcommand.

mod generate;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

/// Builds `.env` files from templates, overrides and command-line values.
#[derive(Parser)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Merge env files and values into a single .env.
    Generate(generate::GenerateArgs),
}

/// Any failure of a subcommand.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Generate(#[from] generate::GenerateError),
}

/// Parses the command line and runs the subcommand it names. A usage error
/// exits here, with clap's message and code.
pub fn run() -> Result<(), Error> {
    // The raw matches too: a subcommand may need what the parsed fields
    // lose, such as where each argument sat on the command line.
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    let (_, subcommand_matches) = matches.subcommand().expect("a subcommand is required");
    match cli.command {
        Command::Generate(args) => {
            generate::run(&args, subcommand_matches)?;
        }
    }
    Ok(())
}
