//! Writing files with the guarantees a `.env` holding secrets needs.

use std::ffi::OsString;
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

/// Writes to a sibling temporary file, then renames it over `path`, so a
/// failure never leaves `path` half-written. An existing `path` keeps its
/// permissions; a new one gets the system default.
///
/// # Implementation
///
/// The permissions go on the temporary file before the contents do, so a
/// `0600` file holding secrets is never readable more widely, even briefly.
pub fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("not a file path"))?;
    let mut tmp_name = OsString::from(".");
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
