//! Drives the real `envik` binary end to end.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BASIC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/basic");

/// A fresh directory under the system temp dir, unique to `test`.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("envik-{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn envik(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_envik"))
        .args(args)
        .output()
        .unwrap()
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn merges_files_and_env_args() {
    let dir = scratch("merges_files_and_env_args");
    let out = dir.join(".env");
    let example = format!("{BASIC}/.env.example");
    let override_ = format!("{BASIC}/.env.override");

    // Each `-e` comes before an `-i` that also sets its key: `-e` must still win.
    let run = envik(&[
        "generate",
        "-e",
        "PORT=445",
        "-i",
        &example,
        "-e",
        "EXTRA=2",
        "-i",
        &override_,
        "-o",
        path(&out),
    ]);

    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        fs::read_to_string(&out).unwrap(),
        "# API\n\
         HOST=localhost\n\
         PORT=445   # porta da API\n\
         \n\
         # Banco\n\
         DB_URL=postgres://localhost/app\n\
         LOG_LEVEL=debug\n\
         EXTRA=2\n",
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn keeps_template_layout() {
    let dir = scratch("keeps_template_layout");
    let template = dir.join("template.env");
    let out = dir.join(".env");
    fs::write(
        &template,
        "# Leading comment\n\
         \n\
         \x20 # indented comment\n\
         HOST = localhost\n\
         PORT=3000   # inline comment\n\
         \n\
         \n\
         # PORT padrao e 3000\n\
         NAME=app\x20\x20\n\
         OPTIONAL= # set me\n\
         LAST=z\n",
    )
    .unwrap();

    let run = envik(&[
        "generate",
        "-i",
        path(&template),
        "-e",
        "HOST=db",
        "-e",
        "PORT=8080",
        "-e",
        "NAME=app",
        "-e",
        "OPTIONAL=1",
        "-o",
        path(&out),
    ]);

    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    // Only each value span changes; `NAME` keeps its value, so its line
    // (trailing blanks included) comes out identical.
    assert_eq!(
        fs::read_to_string(&out).unwrap(),
        "# Leading comment\n\
         \n\
         \x20 # indented comment\n\
         HOST = db\n\
         PORT=8080   # inline comment\n\
         \n\
         \n\
         # PORT padrao e 3000\n\
         NAME=app\x20\x20\n\
         OPTIONAL=1 # set me\n\
         LAST=z\n",
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn errors_leave_output_untouched() {
    let dir = scratch("errors_leave_output_untouched");
    let out = dir.join(".env");
    let original = "KEEP=me\n# untouched\n";
    let example = format!("{BASIC}/.env.example");
    let missing = dir.join("missing.env");

    let cases: [&[&str]; 4] = [
        &["generate", "-i", path(&missing), "-o", path(&out)],
        &["generate", "-o", path(&out)],
        &["generate", "-i", &example, "-e", "1ABC=x", "-o", path(&out)],
        &["generate", "-i", &example, "-e", "=x", "-o", path(&out)],
    ];
    for args in cases {
        fs::write(&out, original).unwrap();

        let run = envik(args);

        assert!(!run.status.success(), "{args:?} succeeded");
        assert!(!run.stderr.is_empty(), "{args:?} printed nothing on stderr");
        assert_eq!(fs::read_to_string(&out).unwrap(), original, "{args:?}");
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, [".env"], "{args:?} left files behind");
    }
    fs::remove_dir_all(&dir).unwrap();
}
