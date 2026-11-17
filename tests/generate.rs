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
         \n\
         # ----\n\
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

#[test]
fn quotes_only_when_needed() {
    let dir = scratch("quotes_only_when_needed");
    let template = dir.join("template.env");
    fs::write(&template, "MSG=x\nSIMPLE=x\nURL=x\nPRICE=x\n").unwrap();

    let run = envik(&[
        "generate",
        "-i",
        path(&template),
        "-e",
        "MSG=a b",
        "-e",
        "SIMPLE=abc",
        "-e",
        "URL=postgres://user@db:5432/app",
        "-e",
        "PRICE=$5",
    ]);

    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap(),
        "MSG='a b'\n\
         SIMPLE=abc\n\
         URL=postgres://user@db:5432/app\n\
         PRICE='$5'\n",
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn uncomments_commented_key() {
    let dir = scratch("uncomments_commented_key");
    let template = dir.join("template.env");
    fs::write(&template, "# DEBUG=\nHOST=x\n#VERBOSE=\n").unwrap();

    let run = envik(&[
        "generate",
        "-i",
        path(&template),
        "-e",
        "DEBUG=1",
        "-e",
        "VERBOSE=yes",
    ]);

    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap(),
        "DEBUG=1\n\
         HOST=x\n\
         VERBOSE=yes\n",
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn literal_values_never_expand() {
    let dir = scratch("literal_values_never_expand");
    let template = dir.join("template.env");
    // `PORT`'s reference is to a missing key, but `-e` replaces it.
    fs::write(&template, "LIT='${X}'\nPORT=${MISSING}\n").unwrap();

    let run = envik(&[
        "generate",
        "-i",
        path(&template),
        "-e",
        "VAR=123",
        "-e",
        "PORT=a${VAR}",
    ]);

    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap(),
        "LIT='${X}'\n\
         PORT='a${VAR}'\n\
         \n\
         # ----\n\
         VAR=123\n",
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn interpolation_errors() {
    let dir = scratch("interpolation_errors");
    let template = dir.join("template.env");
    let out = dir.join(".env");
    let original = "KEEP=me\n# untouched\n";

    let cases = [
        ("A=${X}\n", "envik: `A`: `${X}` is not set\n"),
        ("A=${B}\nB=${A}\n", "envik: circular reference: A → B → A\n"),
        // `X` only references the cycle: no error of its own.
        (
            "X=${A}\nA=${B}\nB=${C}\nC=${A}\n",
            "envik: circular reference: A → B → C → A\n",
        ),
        ("A=${A}\n", "envik: circular reference: A → A\n"),
    ];
    for (contents, expected) in cases {
        fs::write(&template, contents).unwrap();
        fs::write(&out, original).unwrap();

        let run = envik(&["generate", "-i", path(&template), "-o", path(&out)]);

        assert!(!run.status.success(), "{contents:?} succeeded");
        assert_eq!(
            String::from_utf8(run.stderr).unwrap(),
            expected,
            "{contents:?}"
        );
        assert_eq!(fs::read_to_string(&out).unwrap(), original, "{contents:?}");
        let mut entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        entries.sort();
        assert_eq!(
            entries,
            [".env", "template.env"],
            "{contents:?} left files behind"
        );
    }
    fs::remove_dir_all(&dir).unwrap();
}

/// Runs `envik` with `set` added to and `remove` taken out of its environment.
fn envik_env(args: &[&str], set: &[(&str, &str)], remove: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_envik"));
    cmd.args(args);
    for (key, value) in set {
        cmd.env(key, value);
    }
    for key in remove {
        cmd.env_remove(key);
    }
    cmd.output().unwrap()
}

#[test]
fn env_lookup_overrides() {
    let dir = scratch("env_lookup_overrides");
    let template = dir.join("a.env");
    fs::write(&template, "HOST=x\nPORT=1\n").unwrap();

    let run = envik_env(
        &["generate", "-i", path(&template), "-e", "PORT"],
        &[("PORT", "9")],
        &[],
    );

    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).unwrap(), "HOST=x\nPORT=9\n");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn env_lookup_empty_is_value() {
    let dir = scratch("env_lookup_empty_is_value");
    let template = dir.join("a.env");
    fs::write(&template, "PORT=1\n").unwrap();

    let run = envik_env(
        &["generate", "-i", path(&template), "-e", "PORT"],
        &[("PORT", "")],
        &[],
    );

    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).unwrap(), "PORT=\n");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn env_lookup_missing_errors() {
    let dir = scratch("env_lookup_missing_errors");
    let template = dir.join("a.env");
    let out = dir.join(".env");
    let original = "KEEP=me\n# untouched\n";
    fs::write(&template, "PORT=1\n").unwrap();
    fs::write(&out, original).unwrap();

    let run = envik_env(
        &[
            "generate",
            "-i",
            path(&template),
            "-e",
            "PORT",
            "-o",
            path(&out),
        ],
        &[],
        &["PORT"],
    );

    assert!(!run.status.success(), "succeeded");
    assert_eq!(
        String::from_utf8(run.stderr).unwrap(),
        "envik: `PORT` is not set in the environment\n",
    );
    assert_eq!(fs::read_to_string(&out).unwrap(), original);
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    entries.sort();
    assert_eq!(entries, [".env", "a.env"], "left files behind");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn env_lookup_is_literal() {
    let dir = scratch("env_lookup_is_literal");
    let template = dir.join("a.env");
    fs::write(&template, "SECRET=x\n").unwrap();

    let run = envik_env(
        &["generate", "-i", path(&template), "-e", "SECRET"],
        &[("SECRET", "a${X}b")],
        &["X"],
    );

    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    // Single quotes: reading the file back gives the same literal text.
    assert_eq!(String::from_utf8(run.stdout).unwrap(), "SECRET='a${X}b'\n");
    fs::remove_dir_all(&dir).unwrap();
}

/// The value of `name` in `envik`'s `KEY=value` output.
fn value_in<'a>(out: &'a str, name: &str) -> &'a str {
    out.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
        .unwrap_or_else(|| panic!("`{name}` not in {out:?}"))
}

/// The names in `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn rand_values_are_distinct() {
    let dir = scratch("rand_values_are_distinct");
    let template = dir.join("a.env");
    fs::write(
        &template,
        "S=$(rand 32)\nA=$(rand 16)\nB=$(rand 16)\nC=$(rand 8)-$(rand 8)\nD=${S}\n",
    )
    .unwrap();

    let run = envik(&["generate", "-i", path(&template), "--seed-value", "abc"]);

    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let out = String::from_utf8(run.stdout).unwrap();
    let (s, a, b, c, d) = (
        value_in(&out, "S"),
        value_in(&out, "A"),
        value_in(&out, "B"),
        value_in(&out, "C"),
        value_in(&out, "D"),
    );
    assert_eq!(
        [s.len(), a.len(), b.len(), c.len()],
        [32, 16, 16, 17],
        "{out}"
    );
    assert_ne!(a, b, "{out}");
    assert_eq!(&c[8..9], "-", "{out}");
    assert_ne!(&c[..8], &c[9..], "{out}");
    assert_eq!(d, s, "{out}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rand_errors_and_literals() {
    let dir = scratch("rand_errors_and_literals");
    let template = dir.join("a.env");
    let out = dir.join(".env");
    let empty_seed = dir.join("empty.seed");
    let seed = dir.join("good.seed");
    let original = "KEEP=me\n# untouched\n";
    fs::write(&empty_seed, "").unwrap();
    fs::write(&seed, "abc\n").unwrap();

    let ok = "S=$(rand 8)\n";
    // (template, seed arguments, environment set for the child)
    type Case<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, &'a str)]);
    let cases: [Case<'_>; 13] = [
        (ok, &[], &[]),
        ("S=$(rand 0)\n", &["--seed-value", "abc"], &[]),
        ("S=$(rand 1025)\n", &["--seed-value", "abc"], &[]),
        ("S=$(rand x)\n", &["--seed-value", "abc"], &[]),
        ("S=$(nope 1)\n", &["--seed-value", "abc"], &[]),
        ("S=$(rand)\n", &["--seed-value", "abc"], &[]),
        (ok, &["--seed-value", ""], &[]),
        (ok, &["--seed-file", path(&empty_seed)], &[]),
        (ok, &["--seed-env", "ENVIK_SEED"], &[("ENVIK_SEED", "")]),
        (ok, &["--seed-value", "a", "--seed-file", path(&seed)], &[]),
        (
            ok,
            &["--seed-value", "a", "--seed-env", "ENVIK_SEED"],
            &[("ENVIK_SEED", "abc")],
        ),
        (
            ok,
            &["--seed-file", path(&seed), "--seed-env", "ENVIK_SEED"],
            &[("ENVIK_SEED", "abc")],
        ),
        (ok, &["--seed-file-create"], &[]),
    ];
    for (contents, seed_args, set) in cases {
        fs::write(&template, contents).unwrap();
        fs::write(&out, original).unwrap();
        let mut args = vec!["generate", "-i", path(&template), "-o", path(&out)];
        args.extend_from_slice(seed_args);

        let run = envik_env(&args, set, &[]);

        assert!(
            !run.status.success(),
            "{contents:?} {seed_args:?} succeeded"
        );
        assert!(
            !run.stderr.is_empty(),
            "{contents:?} {seed_args:?} printed nothing on stderr"
        );
        assert_eq!(
            fs::read_to_string(&out).unwrap(),
            original,
            "{contents:?} {seed_args:?}"
        );
        assert_eq!(
            entries(&dir),
            [".env", "a.env", "empty.seed", "good.seed"],
            "{contents:?} {seed_args:?} left files behind"
        );
    }

    // No seed is needed: neither of these is a call.
    fs::write(&template, "L='$(rand 8)'\n").unwrap();
    let run = envik(&["generate", "-i", path(&template), "-e", "X=$(rand 8)"]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap(),
        "L='$(rand 8)'\n\n# ----\nX='$(rand 8)'\n"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn seed_file_create_and_reuse() {
    let dir = scratch("seed_file_create_and_reuse");
    let template = dir.join("a.env");
    let seed = dir.join(".seed");
    let out = dir.join(".env");
    let args = [
        "generate",
        "-i",
        path(&template),
        "-o",
        path(&out),
        "--seed-file",
        path(&seed),
        "--seed-file-create",
    ];

    // A failing generation creates no seed.
    fs::write(&template, "S=$(rand 16)\nX=${MISSING}\n").unwrap();
    let run = envik(&args);
    assert!(!run.status.success(), "succeeded");
    assert_eq!(entries(&dir), ["a.env"]);

    fs::write(&template, "S=$(rand 16)\n").unwrap();
    let first = envik(&args);
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let seed_text = fs::read_to_string(&seed).unwrap();
    let first_out = fs::read_to_string(&out).unwrap();
    // 43 base64url characters (32 random bytes) and a newline.
    assert_eq!(seed_text.len(), 44, "{seed_text:?}");
    assert!(seed_text.ends_with('\n'), "{seed_text:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&seed).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let second = envik(&args);
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(fs::read_to_string(&seed).unwrap(), seed_text);
    assert_eq!(fs::read_to_string(&out).unwrap(), first_out);
    // The output really came from the file's seed.
    let reused = envik(&[
        "generate",
        "-i",
        path(&template),
        "--seed-value",
        seed_text.trim_end_matches('\n'),
    ]);
    assert_eq!(String::from_utf8(reused.stdout).unwrap(), first_out);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn seed_from_env() {
    let dir = scratch("seed_from_env");
    let template = dir.join("a.env");
    let missing = dir.join("missing.env");
    fs::write(&template, "S=$(rand 16)\n").unwrap();

    let from_env = envik_env(
        &[
            "generate",
            "-i",
            path(&template),
            "--seed-env",
            "ENVIK_SEED",
        ],
        &[("ENVIK_SEED", "abc")],
        &[],
    );
    let from_value = envik(&["generate", "-i", path(&template), "--seed-value", "abc"]);
    assert!(
        from_env.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&from_env.stderr)
    );
    assert_eq!(
        String::from_utf8(from_env.stdout).unwrap(),
        String::from_utf8(from_value.stdout).unwrap()
    );

    let run = envik_env(
        &[
            "generate",
            "-i",
            path(&template),
            "-i",
            path(&missing),
            "--seed-env",
            "ENVIK_SEED",
        ],
        &[],
        &["ENVIK_SEED"],
    );
    assert!(!run.status.success(), "succeeded");
    assert_eq!(
        String::from_utf8(run.stderr).unwrap(),
        format!(
            "envik: cannot read {}: No such file or directory (os error 2)\n\
             envik: `ENVIK_SEED` is not set in the environment\n",
            path(&missing)
        ),
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn env_override_prefix() {
    let dir = scratch("env_override_prefix");
    let template = dir.join("a.env");
    fs::write(&template, "APP_PASSWORD=dev  # x\n").unwrap();
    let set = [
        ("CI_OVERRIDE_APP_PASSWORD", "s3cr3t"),
        ("CI_OVERRIDE_FOO", "bar"),
    ];

    let with = envik_env(
        &[
            "generate",
            "-i",
            path(&template),
            "--env-override-prefix",
            "CI_OVERRIDE_",
        ],
        &set,
        &[],
    );
    let without = envik_env(&["generate", "-i", path(&template)], &set, &[]);

    assert!(
        with.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&with.stderr)
    );
    assert!(
        without.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&without.stderr)
    );
    // `FOO` is in no input: no line for it, no `# ----` section.
    assert_eq!(
        String::from_utf8(with.stdout).unwrap(),
        "APP_PASSWORD=s3cr3t  # x\n"
    );
    assert_eq!(
        String::from_utf8(without.stdout).unwrap(),
        "APP_PASSWORD=dev  # x\n"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn env_override_prefix_wins_and_turns_on() {
    let dir = scratch("env_override_prefix_wins_and_turns_on");
    let template = dir.join("a.env");
    fs::write(&template, "# DEBUG=\nAPP_PASSWORD=dev\n").unwrap();

    let run = envik_env(
        &[
            "generate",
            "-i",
            path(&template),
            "-e",
            "APP_PASSWORD=x",
            "--env-override-prefix",
            "CI_OVERRIDE_",
        ],
        &[
            ("CI_OVERRIDE_APP_PASSWORD", "y"),
            ("CI_OVERRIDE_DEBUG", "1"),
        ],
        &[],
    );

    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap(),
        "DEBUG=1\nAPP_PASSWORD=y\n"
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn env_override_prefix_invalid() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = scratch("env_override_prefix_invalid");
    let template = dir.join("a.env");
    let out = dir.join(".env");
    let original = "KEEP=me\n# untouched\n";
    fs::write(&template, "A=1\n").unwrap();
    let not_utf8 = || OsString::from_vec(vec![b'x', 0xff, b'y']);
    let run_with = |name: &str| {
        Command::new(env!("CARGO_BIN_EXE_envik"))
            .args([
                "generate",
                "-i",
                path(&template),
                "-o",
                path(&out),
                "--env-override-prefix",
                "CI_OVERRIDE_",
            ])
            .env("CI_OVERRIDE_", "empty")
            .env("CI_OVERRIDE_A-B", "hyphen")
            .env(name, not_utf8())
            .output()
            .unwrap()
    };

    // Invalid names and a non-UTF-8 value for a key no input has: all skipped.
    fs::write(&out, original).unwrap();
    let unused = run_with("CI_OVERRIDE_NOPE");
    assert!(
        unused.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&unused.stderr)
    );
    assert_eq!(fs::read_to_string(&out).unwrap(), "A=1\n");

    // The same value for `A`, which the template has: an error, nothing written.
    fs::write(&out, original).unwrap();
    let used = run_with("CI_OVERRIDE_A");
    assert!(!used.status.success(), "succeeded");
    assert_eq!(
        String::from_utf8(used.stderr).unwrap(),
        "envik: `A`: its value from the environment is not valid UTF-8\n",
    );
    assert_eq!(fs::read_to_string(&out).unwrap(), original);
    assert_eq!(entries(&dir), [".env", "a.env"], "left files behind");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn optional_input_keeps_its_place() {
    let dir = scratch("optional_input_keeps_its_place");
    let a = dir.join("a.env");
    let b = dir.join("b.env");
    let c = dir.join("c.env");
    let missing = dir.join("missing.env");
    fs::write(&a, "K=a\n").unwrap();
    fs::write(&b, "K=b\n").unwrap();
    fs::write(&c, "K=c\n").unwrap();

    // -I between two -i: the later -i wins over it.
    let run = envik(&[
        "generate",
        "-i",
        path(&a),
        "-I",
        path(&missing),
        "-I",
        path(&b),
        "-i",
        path(&c),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).unwrap(), "K=c\n");

    // -I last: it wins over every -i before it.
    let run = envik(&[
        "generate",
        "-i",
        path(&a),
        "-i",
        path(&c),
        "--input-optional",
        path(&b),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).unwrap(), "K=b\n");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn optional_input_still_fails_when_unreadable() {
    let dir = scratch("optional_input_still_fails_when_unreadable");
    let a = dir.join("a.env");
    let out = dir.join(".env");
    let not_a_file = dir.join("folder");
    fs::write(&a, "K=a\n").unwrap();
    fs::write(&out, "OLD=1\n").unwrap();
    fs::create_dir(&not_a_file).unwrap();

    let run = envik(&[
        "generate",
        "-i",
        path(&a),
        "-I",
        path(&not_a_file),
        "-o",
        path(&out),
    ]);

    assert!(!run.status.success(), "succeeded");
    let stderr = String::from_utf8(run.stderr).unwrap();
    assert!(stderr.contains("folder"), "{stderr}");
    assert_eq!(fs::read_to_string(&out).unwrap(), "OLD=1\n");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn required_needs_a_value() {
    let dir = scratch("required_needs_a_value");
    let template = dir.join("a.env");
    let out = dir.join(".env");
    fs::write(&template, "DEBUG=$(required)\nPORT=3000\n").unwrap();
    fs::write(&out, "OLD=1\n").unwrap();

    let run = envik(&["generate", "-i", path(&template), "-o", path(&out)]);
    assert!(!run.status.success(), "succeeded");
    assert_eq!(
        String::from_utf8(run.stderr).unwrap(),
        "envik: `DEBUG` is required: `$(required)`\n\
         envik: set it with -e KEY=VALUE, a later -i, or --env-override-prefix\n",
    );
    assert_eq!(fs::read_to_string(&out).unwrap(), "OLD=1\n");

    let run = envik(&["generate", "-i", path(&template), "-e", "DEBUG=1"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap(),
        "DEBUG=1\nPORT=3000\n"
    );
    fs::remove_dir_all(&dir).unwrap();
}
