# CLAUDE.md

py4uv builds a `py` binary: a portable emulator of the Windows `py` launcher
that delegates to `uv run --no-project [-p X] python …`. Behavior and usage
are documented in README.md; this file covers what matters when changing it.

## Commits

End AI-assisted commit messages with the trailer `Assisted-By: <model name>`
(e.g. `Assisted-By: Claude Fable 5`), without an email address. Do not use
`Co-Authored-By` trailers or `noreply@` addresses.

## Checks and workflow

Every commit is gated by `.githooks/pre-commit` (enable once per clone with
`git config core.hooksPath .githooks`), which runs the typical Rust flow —
fmt --check, check, clippy `-D warnings` (pedantic group is enabled in
Cargo.toml), test, doc with `RUSTDOCFLAGS="-D warnings"` — plus a cross-target
clippy pass. Run the same commands yourself before committing; they must stay
clean.

Cross-target matters: `src/unix.rs` is invisible to every native check on
Windows (and `src/win.rs` on Unix). Lint the other side with
`cargo clippy --all-targets --all-features --target x86_64-unknown-linux-musl -- -D warnings`
(cargo check/clippy don't link, so no cross toolchain is needed).

## Architecture

- `src/main.rs` — shared flow plus version-flag and shebang parsing as pure,
  unit-tested functions.
- `src/win.rs` — argument tail taken raw from `GetCommandLineW` and handed to
  uv verbatim via `raw_arg`; spawns uv and waits, with a console Ctrl handler
  so Ctrl+C reaches the child.
- `src/unix.rs` — argv passed through untouched; `exec`s uv in place of the
  shim, so signals and exit codes need no forwarding.

Both platform modules expose the same interface (`Args::from_env`,
`first_as_str`, `first_as_path`, `skip_first`, `append_to`, `uv_command`,
`run`); keep them in lockstep when extending it.

## Invariants

- Zero external dependencies — std only; the two Win32 externs are declared by
  hand. Don't add crates for convenience.
- On Windows, never re-parse or re-quote the argument tail. Verbatim
  pass-through of `!`, `%`, `^`, and quotes is the core feature (its absence
  was the fatal flaw of the batch-file predecessor). Decoding the tail's
  first token for classification is fine, but must follow the post-2008 CRT
  rules (`decode_next_arg` in win.rs) so classification and what the child
  sees never disagree.
- Mirror the real py launcher: version resolution is explicit flag > shebang >
  `PY_PYTHON` > uv default (bare majors refined via `PY_PYTHON2`/`PY_PYTHON3`),
  shebang interpreter options are forwarded to python, and only the first
  argument is ever treated as a script for shebang purposes.
- Deliberate divergences (keep them, documented in README.md): arch suffixes
  accepted but ignored, `py -h` shows Python's help, no PEP 723 handling,
  `PYLAUNCHER_*` environment variables ignored, shebangs only ever select
  uv-managed interpreters (no venv/absolute-path execution, no env PATH
  search, UTF-8 only), and version requests the shim doesn't recognize are
  passed to `uv run -p` verbatim.

## Verifying end to end

Unit tests cover parsing only; real behavior needs a live check, e.g.
`py -3.12 -c "print('a!b  c%d^e')"` (argument fidelity),
`py script.py` with spaces in the path, and exit-code propagation.
For Linux, build a static binary with
`RUSTFLAGS="-C linker=rust-lld" cargo build --release --target x86_64-unknown-linux-musl`
and run it in WSL; keep uv confined there with `UV_CACHE_DIR` and
`UV_PYTHON_INSTALL_DIR` pointing into `/tmp`.
