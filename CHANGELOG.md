# ChangeLog

## Unreleased

### Changed
- declares Rust 1.86 as its minimum supported version (`rust-version`),
  checked in CI

## 0.1.0 (2026-10-17)

### Added
- first release: a portable `py` launcher shim that runs
  `uv run --no-project [-p <version>] python …`, on Windows and GNU/Linux
- prebuilt binaries on GitHub, and packages on crates.io and PyPI (`py4uv`)
