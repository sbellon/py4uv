//! Unix: argv is already an array, so pass it through untouched and exec uv
//! in place of the shim — signals, terminal control, and exit codes then need
//! no forwarding at all.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

pub struct Args {
    argv: Vec<OsString>,
    /// The not-yet-consumed tail starts here.
    start: usize,
}

impl Args {
    pub fn from_env() -> Self {
        Args {
            argv: std::env::args_os().skip(1).collect(),
            start: 0,
        }
    }

    pub fn first_as_str(&self) -> String {
        self.argv
            .get(self.start)
            .map(|a| a.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn first_as_path(&self) -> PathBuf {
        self.argv
            .get(self.start)
            .cloned()
            .unwrap_or_default()
            .into()
    }

    pub fn skip_first(&mut self) {
        // No-op on an empty tail, in lockstep with the Windows implementation.
        if self.start < self.argv.len() {
            self.start += 1;
        }
    }

    pub fn append_to(&self, cmd: &mut Command) {
        cmd.args(&self.argv[self.start..]);
    }
}

/// uv is found via PATH by `exec` (execvp semantics); nothing to resolve here.
pub fn uv_command() -> Command {
    Command::new("uv")
}

pub fn run(cmd: &mut Command) -> ! {
    use std::os::unix::process::CommandExt;
    // Only returns on error.
    let err = cmd.exec();
    eprintln!("py: failed to run uv: {err}");
    std::process::exit(103);
}
