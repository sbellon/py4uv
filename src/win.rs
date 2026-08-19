//! Windows: take the argument tail from the raw command line (`GetCommandLineW`)
//! and pass it to uv verbatim (`raw_arg`), so no re-quoting can mangle it.
//!
//! Classifying the first token still requires decoding it; `decode_next_arg`
//! follows the post-2008 CRT rules — the same ones Rust's `std::env::args`
//! (and therefore uv and python) use — so the token the shim classifies and
//! the argument the child actually receives are always the same.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

unsafe extern "system" {
    fn GetCommandLineW() -> *const u16;
    fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32;
}

const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

pub struct Args {
    line: Vec<u16>,
    /// The not-yet-consumed tail starts here.
    start: usize,
}

impl Args {
    pub fn from_env() -> Self {
        let line = raw_command_line();
        let start = end_of_argv0(&line);
        Args { line, start }
    }

    fn tail(&self) -> &[u16] {
        &self.line[self.start..]
    }

    pub fn first_as_str(&self) -> String {
        String::from_utf16_lossy(&decode_next_arg(self.tail()).0)
    }

    pub fn first_as_path(&self) -> PathBuf {
        PathBuf::from(OsString::from_wide(&decode_next_arg(self.tail()).0))
    }

    pub fn is_sole_arg(&self) -> bool {
        let tail = self.tail();
        let (_, end) = decode_next_arg(tail);
        skip_ws(tail, end) == tail.len()
    }

    pub fn skip_first(&mut self) {
        let tail = self.tail();
        let (_, end) = decode_next_arg(tail);
        self.start += skip_ws(tail, end);
    }

    pub fn append_to(&self, cmd: &mut Command) {
        if !self.tail().is_empty() {
            cmd.raw_arg(OsString::from_wide(self.tail()));
        }
    }
}

/// Resolve uv from PATH explicitly: with a bare name, `Command::new` would
/// try py.exe's own directory first, letting a stale co-located uv.exe shadow
/// the PATH one. Only uv.exe is considered — a .cmd/.bat shim could not
/// receive the verbatim tail safely (cmd.exe re-parses its command line).
pub fn uv_command() -> Command {
    let from_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(|dir| dir.join("uv.exe"))
            .find(|exe| exe.is_file())
    });
    // No PATH hit: fall back to the bare name, so a uv.exe shipped next to
    // py.exe still works.
    from_path.map_or_else(|| Command::new("uv"), Command::new)
}

fn raw_command_line() -> Vec<u16> {
    unsafe {
        let p = GetCommandLineW();
        let mut len = 0;
        while *p.add(len) != 0 {
            len += 1;
        }
        std::slice::from_raw_parts(p, len).to_vec()
    }
}

/// Skip spaces and tabs starting at `i`.
fn skip_ws(line: &[u16], mut i: usize) -> usize {
    while i < line.len() && (line[i] == SPACE || line[i] == TAB) {
        i += 1;
    }
    i
}

/// Index just past `argv[0]` and the whitespace following it. Like the CRT,
/// argv[0] follows simpler rules than the other arguments: no escapes, and a
/// leading quote runs to the next quote.
fn end_of_argv0(line: &[u16]) -> usize {
    let n = line.len();
    let mut i = 0;
    if line.first() == Some(&QUOTE) {
        i = 1;
        while i < n && line[i] != QUOTE {
            i += 1;
        }
        if i < n {
            i += 1;
        }
    } else {
        while i < n && line[i] != SPACE && line[i] != TAB {
            i += 1;
        }
    }
    skip_ws(line, i)
}

/// Decode the first argument in `s` exactly as the post-2008 CRT (and Rust's
/// std, hence uv) does, returning the decoded code units and the index just
/// past the token. Notably a token continues past a closing quote, so split
/// spellings like `"-3"x` reassemble into the one argument (`-3x`) the child
/// will actually see.
fn decode_next_arg(s: &[u16]) -> (Vec<u16>, usize) {
    let mut arg = Vec::new();
    let mut i = 0;
    let mut in_quotes = false;
    while i < s.len() {
        match s[i] {
            SPACE | TAB if !in_quotes => break,
            BACKSLASH => {
                let run = s[i..].iter().take_while(|&&c| c == BACKSLASH).count();
                i += run;
                if s.get(i) == Some(&QUOTE) {
                    // 2n backslashes + quote: n backslashes, quote handled on
                    // the next round; 2n+1: n backslashes and a literal quote.
                    arg.extend(std::iter::repeat_n(BACKSLASH, run / 2));
                    if run % 2 == 1 {
                        arg.push(QUOTE);
                        i += 1;
                    }
                } else {
                    arg.extend(std::iter::repeat_n(BACKSLASH, run));
                }
            }
            QUOTE => {
                if in_quotes && s.get(i + 1) == Some(&QUOTE) {
                    arg.push(QUOTE); // "" inside quotes: one literal quote
                    i += 2;
                } else {
                    in_quotes = !in_quotes;
                    i += 1;
                }
            }
            c => {
                arg.push(c);
                i += 1;
            }
        }
    }
    (arg, i)
}

pub fn run(cmd: &mut Command) -> ! {
    // Stay alive on Ctrl+C so the child handles it (KeyboardInterrupt in the
    // REPL) and we can still report its exit code. A handler routine is used
    // instead of SetConsoleCtrlHandler(NULL, ...) because the NULL "ignore"
    // flag would be inherited by the python child.
    unsafe extern "system" fn ignore_ctrl(_ctrl_type: u32) -> i32 {
        1
    }
    unsafe {
        SetConsoleCtrlHandler(Some(ignore_ctrl), 1);
    }

    match cmd.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(103)),
        Err(err) => {
            eprintln!("py: failed to run uv: {err}");
            std::process::exit(103);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn decoded(s: &str) -> (String, usize) {
        let (arg, end) = decode_next_arg(&wide(s));
        (String::from_utf16(&arg).unwrap(), end)
    }

    #[test]
    fn crt_decoding() {
        assert_eq!(decoded(""), (String::new(), 0));
        assert_eq!(decoded("-3.12 script.py"), ("-3.12".into(), 5));
        assert_eq!(decoded(r#""-3"x script.py"#), ("-3x".into(), 5));
        assert_eq!(decoded(r#"""-3.12"" script.py"#), ("-3.12".into(), 9));
        assert_eq!(decoded(r#""a b".py rest"#), ("a b.py".into(), 8));
        assert_eq!(
            decoded(r#""C:\Program Files"\Vendor\tool.py x"#),
            (r"C:\Program Files\Vendor\tool.py".into(), 33)
        );
        assert_eq!(decoded(r#"script.py"""#), ("script.py".into(), 11));
        assert_eq!(decoded(r#"a\"b c"#), (r#"a"b"#.into(), 4));
        assert_eq!(decoded(r#"a\\"b c" d"#), (r"a\b c".into(), 8));
        assert_eq!(decoded(r#""a""b" c"#), (r#"a"b"#.into(), 6));
    }

    #[test]
    fn argv0_and_skipping() {
        assert_eq!(end_of_argv0(&wide("py -3")), 3);
        assert_eq!(end_of_argv0(&wide(r#""C:\tools\py.exe"  -3"#)), 19);
        assert_eq!(end_of_argv0(&wide("py")), 2);

        let mut args = Args {
            line: wide(r#"py ""-3.12"" script.py"#),
            start: 3,
        };
        assert_eq!(args.first_as_str(), "-3.12");
        assert!(!args.is_sole_arg());
        args.skip_first();
        assert_eq!(String::from_utf16(args.tail()).unwrap(), "script.py");
        assert!(args.is_sole_arg());
        args.skip_first();
        assert!(args.tail().is_empty());
        args.skip_first(); // no-op on an empty tail
        assert!(args.tail().is_empty());
    }
}
