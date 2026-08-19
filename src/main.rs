// py4uv - "py" launcher shim backed by uv (no Python installation needed).
//
// Translates:
//   py [-3 | -3.12 | -3.12-64 | -V:3.12] <python args...>
// into:
//   uv run --no-project [-p <version>] python <python args...>
//
// On Windows the argument tail is taken from the raw command line
// (GetCommandLineW) and appended to uv's command line verbatim (raw_arg), so
// quoting, '!', '%', '^' etc. survive exactly as typed. On Unix argv is
// passed through as-is and uv is exec'd in place of the shim.
//
// Version resolution order (mirrors the real py launcher):
//   explicit flag > script shebang > PY_PYTHON env var > uv default
//   (uv default = UV_PYTHON / .python-version / latest managed install),
// and a bare major version from any of those is refined via
// PY_PYTHON2/PY_PYTHON3. Interpreter options trailing a python shebang
// ("#!/usr/bin/python3 -u") are passed to python, like the real launcher.
//
// py -0 / -0p / --list as the sole argument map to
// `uv python list --only-installed`.

#[cfg(windows)]
#[path = "win.rs"]
mod platform;
#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(not(any(windows, unix)))]
compile_error!("py4uv supports Windows and Unix platforms only");

use std::io::{BufRead, Read};
use std::path::Path;

fn main() {
    let mut args = platform::Args::from_env();

    let first = args.first_as_str();
    // Like the real launcher, list flags only count when they are the sole
    // argument; otherwise they go to python like any other option.
    if args.is_sole_arg() && matches!(first.as_str(), "-0" | "-0p" | "--list" | "--list-paths") {
        let mut cmd = platform::uv_command();
        cmd.args(["python", "list", "--only-installed"]);
        platform::run(&mut cmd);
    }

    let mut version = parse_version_flag(&first);
    let mut python_opts = Vec::new();
    if version.is_some() {
        args.skip_first();
    } else if !first.is_empty() && !first.starts_with('-') {
        // Like the real launcher, only the first argument is considered a script.
        if let Some(shebang) = script_shebang(&args.first_as_path()) {
            version = shebang.version;
            python_opts = shebang.options;
        }
    }
    let mut version = version.or_else(|| env_version("PY_PYTHON"));
    // A bare major version (from flag, shebang, or PY_PYTHON) is refined via
    // PY_PYTHON2/PY_PYTHON3, like the real launcher.
    if version.as_deref() == Some("2") {
        version = env_version("PY_PYTHON2").or(version);
    } else if version.as_deref() == Some("3") {
        version = env_version("PY_PYTHON3").or(version);
    }

    let mut cmd = platform::uv_command();
    cmd.args(["run", "--no-project"]);
    if let Some(v) = &version {
        cmd.args(["-p", v]);
    }
    cmd.arg("python");
    // Shebang interpreter options go before the script (the tail's first token).
    cmd.args(&python_opts);
    args.append_to(&mut cmd);
    platform::run(&mut cmd);
}

/// Matches -2, -3, -3.12 and -V:3.12 / -V:Company/3.12, each optionally with
/// a -32/-64/-arm64 suffix (accepted but ignored, in any case), returning the
/// version to pass to `uv run -p`.
fn parse_version_flag(tok: &str) -> Option<String> {
    if let Some(rest) = tok.strip_prefix("-V:") {
        return normalize_version_tag(rest);
    }
    let body = tok.strip_prefix('-')?;
    let body = strip_arch_suffix(body);
    if !body.starts_with(['2', '3']) {
        return None;
    }
    let ok = match &body[1..] {
        "" => true,
        rest => rest
            .strip_prefix('.')
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())),
    };
    ok.then(|| body.to_string())
}

/// A `-V:` or `PY_PYTHON` style tag — "3.12", "3.12-64", "Company/3.12" —
/// normalized to the version for `uv run -p`; None if it isn't a version.
fn normalize_version_tag(tag: &str) -> Option<String> {
    let ver = tag.rsplit(['/', '\\']).next().unwrap();
    let ver = strip_arch_suffix(ver);
    is_dotted_number(ver).then(|| ver.to_string())
}

/// Strip one -32/-64/-arm64 architecture suffix, case-insensitively. Arch
/// selection is accepted but ignored (uv manages one build per version).
fn strip_arch_suffix(s: &str) -> &str {
    let bytes = s.as_bytes();
    for suffix in [b"-32".as_slice(), b"-64", b"-arm64"] {
        if bytes.len() >= suffix.len()
            && bytes[bytes.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
        {
            // The suffix is pure ASCII, so the cut is a char boundary.
            return &s[..s.len() - suffix.len()];
        }
    }
    s
}

fn is_dotted_number(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Version from an environment variable, normalized like a -V: tag. A set but
/// unusable value aborts: passed through it would derail uv's own flag
/// parsing, and ignored it would silently run the wrong interpreter.
fn env_version(var: &str) -> Option<String> {
    let val = std::env::var(var).ok()?;
    if val.is_empty() {
        return None;
    }
    normalize_version_tag(&val).or_else(|| {
        eprintln!("py: {var}={val} is not a version like 3 or 3.12");
        std::process::exit(103);
    })
}

/// A recognized python shebang: optional version, plus trailing interpreter
/// options, which the real launcher forwards to python.
#[derive(Debug, PartialEq, Eq)]
struct Shebang {
    version: Option<String>,
    options: Vec<String>,
}

/// Read and parse the script's shebang line. Unreadable or not a file: fall
/// through and let python report the error.
fn script_shebang(path: &Path) -> Option<Shebang> {
    // Generous cap for one line; a line still unterminated at the cap is
    // ignored rather than misparsed at the cut.
    const CAP: usize = 4096;
    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    std::io::BufReader::new(file.take(CAP as u64))
        .read_until(b'\n', &mut buf)
        .ok()?;
    if buf.len() == CAP && buf.last() != Some(&b'\n') {
        return None;
    }
    let head = buf.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&buf);
    parse_shebang(String::from_utf8_lossy(head).trim_end())
}

/// Parse a `#!` line. Recognized when the interpreter's basename is
/// `python[X[.Y]]` — optionally via `env`, with an `.exe` extension, or with
/// an ignored arch suffix; anything else yields None.
fn parse_shebang(line: &str) -> Option<Shebang> {
    let rest = line.strip_prefix("#!")?;
    let mut tokens = rest.split_whitespace();
    let mut interpreter = tokens.next()?;
    if trim_exe(basename(interpreter)).eq_ignore_ascii_case("env") {
        interpreter = tokens.next()?;
    }
    let name = trim_exe(basename(interpreter));
    let tag = strip_arch_suffix(name.strip_prefix("python")?);
    let version = if tag.is_empty() {
        None
    } else if is_dotted_number(tag) {
        Some(tag.to_string())
    } else {
        return None; // e.g. "pythonw": not a plain python interpreter
    };
    Some(Shebang {
        version,
        options: tokens.map(str::to_string).collect(),
    })
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap()
}

fn trim_exe(name: &str) -> &str {
    let bytes = name.as_bytes();
    if bytes.len() > 4 && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".exe") {
        &name[..name.len() - 4]
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_flags() {
        assert_eq!(parse_version_flag("-3"), Some("3".into()));
        assert_eq!(parse_version_flag("-2"), Some("2".into()));
        assert_eq!(parse_version_flag("-3.12"), Some("3.12".into()));
        assert_eq!(parse_version_flag("-2.7"), Some("2.7".into()));
        assert_eq!(parse_version_flag("-3.12-64"), Some("3.12".into()));
        assert_eq!(parse_version_flag("-3-32"), Some("3".into()));
        assert_eq!(parse_version_flag("-3.10-arm64"), Some("3.10".into()));
        assert_eq!(parse_version_flag("-3.12-ARM64"), Some("3.12".into()));
        assert_eq!(parse_version_flag("-V:3.12"), Some("3.12".into()));
        assert_eq!(parse_version_flag("-V:3.12-64"), Some("3.12".into()));
        assert_eq!(parse_version_flag("-V:3.13-arm64"), Some("3.13".into()));
        assert_eq!(parse_version_flag("-V:Astral/3.13"), Some("3.13".into()));
        assert_eq!(parse_version_flag("-V:Astral/3.13-64"), Some("3.13".into()));
        assert_eq!(parse_version_flag("-V:3.13.2"), Some("3.13.2".into()));
    }

    #[test]
    fn non_version_flags() {
        assert_eq!(parse_version_flag(""), None);
        assert_eq!(parse_version_flag("-"), None);
        assert_eq!(parse_version_flag("-32"), None);
        assert_eq!(parse_version_flag("-4"), None);
        assert_eq!(parse_version_flag("-3."), None);
        assert_eq!(parse_version_flag("-3x"), None);
        assert_eq!(parse_version_flag("-c"), None);
        assert_eq!(parse_version_flag("-m"), None);
        assert_eq!(parse_version_flag("-V"), None);
        assert_eq!(parse_version_flag("-V:"), None);
        assert_eq!(parse_version_flag("-V:Astral/"), None);
        assert_eq!(parse_version_flag("script.py"), None);
    }

    #[test]
    fn version_tags() {
        assert_eq!(normalize_version_tag("3.12"), Some("3.12".into()));
        assert_eq!(normalize_version_tag("3.12-64"), Some("3.12".into()));
        assert_eq!(normalize_version_tag("3.12-ARM64"), Some("3.12".into()));
        assert_eq!(
            normalize_version_tag("Astral/3.13-arm64"),
            Some("3.13".into())
        );
        assert_eq!(normalize_version_tag("foo"), None);
        assert_eq!(normalize_version_tag(""), None);
        assert_eq!(normalize_version_tag("-64"), None);
    }

    fn sb(version: Option<&str>, options: &[&str]) -> Shebang {
        Shebang {
            version: version.map(String::from),
            options: options.iter().map(|&s| s.to_string()).collect(),
        }
    }

    #[test]
    fn shebang_lines() {
        assert_eq!(
            parse_shebang("#!/usr/bin/env python3.10"),
            Some(sb(Some("3.10"), &[]))
        );
        assert_eq!(
            parse_shebang("#!/usr/bin/python3"),
            Some(sb(Some("3"), &[]))
        );
        assert_eq!(
            parse_shebang("#!python3.12 -u"),
            Some(sb(Some("3.12"), &["-u"]))
        );
        assert_eq!(
            parse_shebang("#!/usr/bin/env python3.12 -u -X utf8"),
            Some(sb(Some("3.12"), &["-u", "-X", "utf8"]))
        );
        assert_eq!(parse_shebang("#!/usr/bin/env python"), Some(sb(None, &[])));
        assert_eq!(
            parse_shebang("#!/usr/bin/python -u"),
            Some(sb(None, &["-u"]))
        );
        assert_eq!(
            parse_shebang("#!/usr/bin/python3.7-32"),
            Some(sb(Some("3.7"), &[]))
        );
    }

    #[test]
    fn shebang_matches_basename_only() {
        // Versioned directory components must not hijack the version.
        assert_eq!(
            parse_shebang("#!/opt/python2-tools/bin/python3"),
            Some(sb(Some("3"), &[]))
        );
        assert_eq!(
            parse_shebang("#!/home/me/mypython3env/bin/python"),
            Some(sb(None, &[]))
        );
        assert_eq!(
            parse_shebang(r"#!C:\Python312\python.exe"),
            Some(sb(None, &[]))
        );
    }

    #[test]
    fn non_python_shebangs() {
        assert_eq!(parse_shebang("#!/bin/sh"), None);
        assert_eq!(parse_shebang("#!/usr/bin/pythonw3.12"), None);
        assert_eq!(parse_shebang("#!/usr/bin/env"), None);
        assert_eq!(parse_shebang("#!"), None);
        assert_eq!(parse_shebang("print('hi')"), None);
    }
}
