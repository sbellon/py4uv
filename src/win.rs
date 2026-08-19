//! Windows: take the argument tail from the raw command line (`GetCommandLineW`)
//! and pass it to uv verbatim (`raw_arg`), so no re-quoting can mangle it.

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
        String::from_utf16_lossy(peek_token(self.tail()))
            .trim_matches('"')
            .to_string()
    }

    pub fn first_as_path(&self) -> PathBuf {
        let mut tok = peek_token(self.tail());
        if tok.first() == Some(&QUOTE) {
            tok = &tok[1..];
        }
        if tok.last() == Some(&QUOTE) {
            tok = &tok[..tok.len() - 1];
        }
        PathBuf::from(OsString::from_wide(tok))
    }

    pub fn skip_first(&mut self) {
        let tail = self.tail();
        let mut i = peek_token(tail).len();
        while i < tail.len() && (tail[i] == SPACE || tail[i] == TAB) {
            i += 1;
        }
        self.start += i;
    }

    pub fn append_to(&self, cmd: &mut Command) {
        if !self.tail().is_empty() {
            cmd.raw_arg(OsString::from_wide(self.tail()));
        }
    }
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

/// Index just past `argv[0]` and the whitespace following it.
fn end_of_argv0(line: &[u16]) -> usize {
    let n = line.len();
    let mut i = 0;
    if n > 0 && line[0] == QUOTE {
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
    while i < n && (line[i] == SPACE || line[i] == TAB) {
        i += 1;
    }
    i
}

/// First whitespace-delimited token; an opening quote runs to its closing quote.
fn peek_token(s: &[u16]) -> &[u16] {
    let n = s.len();
    if n == 0 {
        return s;
    }
    let mut i = 0;
    if s[0] == QUOTE {
        i = 1;
        while i < n && s[i] != QUOTE {
            i += 1;
        }
        if i < n {
            i += 1;
        }
    } else {
        while i < n && s[i] != SPACE && s[i] != TAB {
            i += 1;
        }
    }
    &s[..i]
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
