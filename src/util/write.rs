//! Writing files with the guarantees a `.env` holding secrets needs.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Creates `path` with `contents`, readable by its owner only, failing if it
/// already exists rather than replacing it.
pub fn create_private(path: &Path, contents: &str) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(contents.as_bytes())
}
