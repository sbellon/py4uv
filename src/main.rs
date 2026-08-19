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
// PY_PYTHON2/PY_PYTHON3 — except from a -V: tag, which the real launcher
// treats as exact. Interpreter options trailing a python shebang
// ("#!/usr/bin/python3 -u") are passed to python, like the real launcher.
//
// py -0 / -0p / --list / --list-paths as the first argument map to
// `uv python list --only-installed`; further arguments are ignored, like
// the real launcher.

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
    // Like the real launcher, a list flag is recognized as the first
    // argument; it lists and exits, and further arguments are ignored.
    if matches!(first.as_str(), "-0" | "-0p" | "--list" | "--list-paths") {
        let mut cmd = platform::uv_command();
        cmd.args(["python", "list", "--only-installed"]);
        platform::run(&mut cmd);
    }

    let mut version = parse_version_flag(&first);
    // The real launcher treats a -V: tag as exact and never refines it.
    let exact_tag = version.is_some() && first.starts_with("-V:");
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
    version = version.or_else(|| env_version("PY_PYTHON"));
    // A bare major version (from flag, shebang, or PY_PYTHON) is refined via
    // PY_PYTHON2/PY_PYTHON3, like the real launcher.
    if !exact_tag {
        if version.as_deref() == Some("2") {
            version = env_version("PY_PYTHON2").or(version);
        } else if version.as_deref() == Some("3") {
            version = env_version("PY_PYTHON3").or(version);
        }
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

/// Matches -2, -3, -3.12 and `-V:` requests (`-V:3.12`, `-V:PyPy/3.10`,
/// `-V:3.13t`), the former optionally with a -32/-64/-arm64 suffix (accepted
/// but ignored, in any case), returning the request to pass to `uv run -p`.
fn parse_version_flag(tok: &str) -> Option<String> {
    if let Some(rest) = tok.strip_prefix("-V:") {
        return resolve_version_request(rest);
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

/// A `-V:` or `PY_PYTHON*` version request, resolved to a `uv run -p`
/// argument. Dotted versions (with an ignored arch suffix) pass through;
/// `Company/version` tags map to uv's `implementation@version` form, with
/// `PythonCore` meaning plain `CPython` (an unknown company then fails
/// loudly in uv instead of silently running the wrong one). Anything else
/// the shim does not understand is handed to uv verbatim, so uv-native
/// requests ("3.13t", "pypy3.10", an interpreter path) keep working. Empty
/// requests and requests starting with `-`, which could derail uv's flag
/// parsing, yield None.
fn resolve_version_request(tag: &str) -> Option<String> {
    if tag.is_empty() || tag.starts_with('-') {
        return None;
    }
    let ver = strip_arch_suffix(tag);
    if is_dotted_number(ver) {
        return Some(ver.to_string());
    }
    if let Some((company, ver)) = tag.rsplit_once(['/', '\\']) {
        let ver = strip_arch_suffix(ver);
        if is_dotted_number(ver) {
            return Some(if company.eq_ignore_ascii_case("PythonCore") {
                ver.to_string()
            } else {
                let company = company.to_ascii_lowercase();
                format!("{company}@{ver}")
            });
        }
    }
    Some(tag.to_string())
}

/// Strip one -32/-64/-arm64 architecture suffix, case-insensitively. Arch
/// selection is accepted but ignored (uv manages one build per version).
fn strip_arch_suffix(s: &str) -> &str {
    ["-32", "-64", "-arm64"]
        .iter()
        .find_map(|suffix| strip_suffix_ignore_case(s, suffix))
        .unwrap_or(s)
}

fn is_dotted_number(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Version request from an environment variable, resolved like a -V: tag. A
/// set but unusable value (non-Unicode, or starting with `-`) aborts: passed
/// through it could derail uv's own flag parsing, and ignored it would
/// silently run the wrong interpreter.
fn env_version(var: &str) -> Option<String> {
    let val = std::env::var_os(var)?;
    if val.is_empty() {
        return None;
    }
    let Some(val) = val.to_str() else {
        eprintln!("py: {var} is set but is not valid Unicode");
        std::process::exit(103);
    };
    resolve_version_request(val).or_else(|| {
        eprintln!("py: {var}={val} is not a usable version request");
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
    // Generous cap for one line; a line still unterminated past the cap is
    // ignored rather than misparsed at the cut. Reading one byte beyond the
    // cap distinguishes that from a complete line ending exactly at it.
    const CAP: usize = 4096;
    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    std::io::BufReader::new(file.take(CAP as u64 + 1))
        .read_until(b'\n', &mut buf)
        .ok()?;
    if buf.len() > CAP {
        return None;
    }
    let head = buf.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&buf);
    // Strict UTF-8: a mis-encoded line is ignored rather than parsed with
    // mangled bytes (no ANSI-code-page fallback, see README).
    parse_shebang(std::str::from_utf8(head).ok()?.trim_end())
}

/// Parse a `#!` line. Recognized when the interpreter's basename is
/// `python[X[.Y]]` in any case — optionally via `env`, with an `.exe`
/// extension, or with an ignored arch suffix; anything else yields None.
fn parse_shebang(line: &str) -> Option<Shebang> {
    let rest = line.strip_prefix("#!")?;
    let mut tokens = shebang_tokens(rest);
    let mut interpreter = tokens.next()?;
    if trim_exe(basename(&interpreter)).eq_ignore_ascii_case("env") {
        interpreter = tokens.next()?;
    }
    let name = trim_exe(basename(&interpreter));
    let tag = strip_arch_suffix(strip_prefix_ignore_case(name, "python")?);
    let version = if tag.is_empty() {
        None
    } else if is_dotted_number(tag) {
        Some(tag.to_string())
    } else {
        return None; // e.g. "pythonw": not a plain python interpreter
    };
    Some(Shebang {
        version,
        options: tokens.collect(),
    })
}

/// Whitespace-separated tokens, except that double quotes group and are
/// stripped (`-W "a b"` is the two tokens `-W` and `a b`), so quoted
/// interpreter paths and option arguments with spaces survive.
fn shebang_tokens(s: &str) -> impl Iterator<Item = String> {
    let mut chars = s.chars().peekable();
    std::iter::from_fn(move || {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        chars.peek()?;
        let mut token = String::new();
        let mut in_quotes = false;
        while let Some(&c) = chars.peek() {
            if c == '"' {
                in_quotes = !in_quotes;
            } else if c.is_whitespace() && !in_quotes {
                break;
            } else {
                token.push(c);
            }
            chars.next();
        }
        Some(token)
    })
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap()
}

fn trim_exe(name: &str) -> &str {
    // A name that is nothing but ".exe" is kept as-is.
    match strip_suffix_ignore_case(name, ".exe") {
        Some(stem) if !stem.is_empty() => stem,
        _ => name,
    }
}

/// `strip_prefix` up to ASCII case. The prefix must be ASCII, so the cut is
/// a char boundary.
fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len()
        && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// `strip_suffix` up to ASCII case. The suffix must be ASCII, so the cut is
/// a char boundary.
fn strip_suffix_ignore_case<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    if s.len() >= suffix.len()
        && s.as_bytes()[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
    {
        Some(&s[..s.len() - suffix.len()])
    } else {
        None
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
        assert_eq!(parse_version_flag("-V:3.13.2"), Some("3.13.2".into()));
        assert_eq!(
            parse_version_flag("-V:PythonCore/3.12"),
            Some("3.12".into())
        );
        assert_eq!(parse_version_flag("-V:PyPy/3.10"), Some("pypy@3.10".into()));
        assert_eq!(
            parse_version_flag("-V:Astral/3.13-64"),
            Some("astral@3.13".into())
        );
        // Requests the shim doesn't understand go to uv verbatim.
        assert_eq!(parse_version_flag("-V:3.13t"), Some("3.13t".into()));
        assert_eq!(parse_version_flag("-V:pypy3.10"), Some("pypy3.10".into()));
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
        assert_eq!(parse_version_flag("-V:-u"), None);
        assert_eq!(parse_version_flag("script.py"), None);
    }

    #[test]
    fn version_requests() {
        assert_eq!(resolve_version_request("3.12"), Some("3.12".into()));
        assert_eq!(resolve_version_request("3.12-64"), Some("3.12".into()));
        assert_eq!(resolve_version_request("3.12-ARM64"), Some("3.12".into()));
        assert_eq!(
            resolve_version_request("PythonCore/3.12"),
            Some("3.12".into())
        );
        assert_eq!(
            resolve_version_request(r"pythoncore\3.12-64"),
            Some("3.12".into())
        );
        assert_eq!(
            resolve_version_request("PyPy/3.10"),
            Some("pypy@3.10".into())
        );
        assert_eq!(
            resolve_version_request("Astral/3.13-arm64"),
            Some("astral@3.13".into())
        );
        // Anything else is uv's to resolve (free-threaded and pre-release
        // versions, implementation names, interpreter paths, ...).
        assert_eq!(resolve_version_request("3.13t"), Some("3.13t".into()));
        assert_eq!(
            resolve_version_request("3.14.0rc1"),
            Some("3.14.0rc1".into())
        );
        assert_eq!(resolve_version_request("pypy3.10"), Some("pypy3.10".into()));
        assert_eq!(
            resolve_version_request(r"C:\Python312\python.exe"),
            Some(r"C:\Python312\python.exe".into())
        );
        // ... except empty or flag-like requests, which would derail uv.
        assert_eq!(resolve_version_request(""), None);
        assert_eq!(resolve_version_request("-64"), None);
        assert_eq!(resolve_version_request("-p"), None);
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
        // Double quotes group and are stripped, for paths and options alike.
        assert_eq!(
            parse_shebang(r#"#!"C:\Program Files\Python312\python.exe" -u"#),
            Some(sb(None, &["-u"]))
        );
        assert_eq!(
            parse_shebang(r#"#!/usr/bin/python3 -W "ignore:deprecated call""#),
            Some(sb(Some("3"), &["-W", "ignore:deprecated call"]))
        );
        // The basename matches in any case, like Windows filesystems.
        assert_eq!(
            parse_shebang(r"#!C:\Python312\PYTHON.EXE -u"),
            Some(sb(None, &["-u"]))
        );
        assert_eq!(
            parse_shebang("#!/usr/bin/env Python3"),
            Some(sb(Some("3"), &[]))
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
        assert_eq!(parse_shebang("#!/usr/bin/PYTHONW3.12"), None);
        assert_eq!(parse_shebang("#!/usr/bin/env"), None);
        assert_eq!(parse_shebang("#!"), None);
        assert_eq!(parse_shebang("print('hi')"), None);
    }
}
