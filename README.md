# py4uv

A portable `py` launcher shim, backed by [uv](https://docs.astral.sh/uv/) — no
Python installation needed. `py -3.10 …`, `py -3.12 …` etc. work transparently
on Windows, GNU/Linux, and macOS; uv downloads missing interpreters on demand.

## What it does

Translates

```
py [-3 | -3.12 | -3.12-64 | -V:3.12] <python args...>
```

into

```
uv run --no-project [-p <version>] python <python args...>
```

On Windows the argument tail is taken from the raw command line
(`GetCommandLineW`) and appended to uv's command line verbatim (`raw_arg`), so
quoting, `!`, `%`, `^` etc. survive exactly as typed; Ctrl+C is forwarded to
Python (the REPL gets `KeyboardInterrupt` instead of the shim dying) and exit
codes propagate. On Unix argv is passed through untouched and uv is `exec`'d
in place of the shim, so signals and exit codes need no forwarding at all.

Version resolution order mirrors the real py launcher:

1. explicit flag (`-3`, `-3.12`, `-3.12-64`, `-V:3.12`)
2. shebang line of the script given as first argument
   (`#!/usr/bin/env python3.10`); interpreter options trailing it
   (`#!/usr/bin/python3 -u`) are passed on to python
3. `PY_PYTHON` environment variable
4. uv's default (`UV_PYTHON` / `.python-version` / latest managed install)

A bare major version from any of these (`py -3`, `#!/usr/bin/python3`,
`PY_PYTHON=3`) is refined through `PY_PYTHON3`/`PY_PYTHON2`, like the real
launcher.

`py -0` / `-0p` / `--list` as the sole argument map to
`uv python list --only-installed` (with further arguments they go to python,
like the real launcher).

Deliberate divergences from the real launcher: architecture suffixes
(`-32`/`-64`/`-arm64`) are accepted but ignored, `py -h` shows Python's help
rather than launcher help, `py script.py` runs the script with plain
`python` (no PEP 723 inline-dependency handling — use `uv run script.py` for
that), and the `PYLAUNCHER_*` environment variables (`PYLAUNCHER_DRYRUN`,
`PYLAUNCHER_DEBUG`, `PYLAUNCHER_ALLOW_INSTALL`, …) are ignored. On Windows,
uv must be a real `uv.exe` — found on `PATH`, or failing that next to
`py.exe` — since a `.cmd`/`.bat` shim would have cmd.exe re-parse the
verbatim argument tail.

## Checks

The typical Rust flow must stay clean (clippy runs with the `pedantic`
group enabled via `[lints.clippy]` in `Cargo.toml`):

```
cargo fmt --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo doc --no-deps --all-features
```

plus a clippy pass for the other platform's module via a cross target,
e.g. from Windows:

```
cargo clippy --all-targets --all-features --target x86_64-unknown-linux-musl -- -D warnings
```

A pre-commit hook runs all of it automatically; enable it once per clone with:

```
git config core.hooksPath .githooks
```

## Build & install

Anywhere with a Rust toolchain:

```
cargo install --path .
```

installs `py` into `~/.cargo/bin`. Or build and copy manually, e.g. on Windows:

```
cargo build --release
copy target\release\py.exe %USERPROFILE%\.local\bin\py.exe
```

## License

Licensed under the [MIT license](LICENSE).
