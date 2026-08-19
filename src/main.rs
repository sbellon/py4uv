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
//   (uv default = UV_PYTHON / .python-version / latest managed install).
//
// py -0 / -0p / --list map to `uv python list --only-installed`.

#[cfg(windows)]
#[path = "win.rs"]
mod platform;
#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(not(any(windows, unix)))]
compile_error!("py4uv supports Windows and Unix platforms only");

use std::io::Read;
use std::path::Path;
use std::process::Command;

fn main() {
    let mut args = platform::Args::from_env();

    let first = args.first_as_str();
    if matches!(first.as_str(), "-0" | "-0p" | "--list" | "--list-paths") {
        let mut cmd = Command::new("uv");
        cmd.args(["python", "list", "--only-installed"]);
        platform::run(&mut cmd);
    }

    let mut version = parse_version_flag(&first);
    if version.is_some() {
        args.skip_first();
    } else if !first.is_empty() && !first.starts_with('-') {
        // Like the real launcher, only the first argument is considered a script.
        version = shebang_version(&args.first_as_path());
    }
    let version = version
        .or_else(|| std::env::var("PY_PYTHON").ok())
        .filter(|v| !v.is_empty());

    let mut cmd = Command::new("uv");
    cmd.args(["run", "--no-project"]);
    if let Some(v) = &version {
        cmd.args(["-p", v]);
    }
    cmd.arg("python");
    args.append_to(&mut cmd);
    platform::run(&mut cmd);
}

/// Matches -2, -3, -3.12 (optionally with a -32/-64/-arm64 suffix, which is
/// accepted but ignored) and -V:3.12 / -V:Company/3.12, returning the
/// version to pass to `uv run -p`.
fn parse_version_flag(tok: &str) -> Option<String> {
    if let Some(rest) = tok.strip_prefix("-V:") {
        let ver = rest.rsplit(['/', '\\']).next().unwrap();
        return is_dotted_number(ver).then(|| ver.to_string());
    }
    let body = tok.strip_prefix('-')?;
    let body = body
        .strip_suffix("-32")
        .or_else(|| body.strip_suffix("-64"))
        .or_else(|| body.strip_suffix("-arm64"))
        .unwrap_or(body);
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

fn is_dotted_number(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn shebang_version(path: &Path) -> Option<String> {
    // Unreadable or not a file: fall through and let python report the error.
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 512];
    let n = file.read(&mut buf).ok()?;
    let head = buf[..n].strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&buf[..n]);
    let line_end = head.iter().position(|&b| b == b'\n').unwrap_or(head.len());
    let line = String::from_utf8_lossy(&head[..line_end]);
    if !line.starts_with("#!") {
        return None;
    }
    python_version_in(&line)
}

/// First "python" followed by digits, e.g. "#!/usr/bin/env python3.10" -> "3.10".
fn python_version_in(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(pos) = line[from..].find("python") {
        let start = from + pos + "python".len();
        let mut i = start;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i > start {
            if i < bytes.len() && bytes[i] == b'.' {
                let frac = i + 1;
                let mut j = frac;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > frac {
                    i = j;
                }
            }
            return Some(line[start..i].to_string());
        }
        from = start;
    }
    None
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
        assert_eq!(parse_version_flag("-V:3.12"), Some("3.12".into()));
        assert_eq!(parse_version_flag("-V:Astral/3.13"), Some("3.13".into()));
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
    fn shebang_lines() {
        assert_eq!(
            python_version_in("#!/usr/bin/env python3.10"),
            Some("3.10".into())
        );
        assert_eq!(python_version_in("#!/usr/bin/python3"), Some("3".into()));
        assert_eq!(python_version_in("#!python3.12 -u"), Some("3.12".into()));
        assert_eq!(python_version_in("#!/usr/bin/env python"), None);
        assert_eq!(python_version_in("#!/bin/sh"), None);
    }
}
