//! The `envik` binary. The command line and all of the I/O live in [`cli`];
//! the generation itself is [`envik::Generator`], which never touches the
//! disk.

mod cli;
mod util;

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(error) = cli::run() {
        for line in error.to_string().lines() {
            eprintln!("envik: {line}");
        }
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
