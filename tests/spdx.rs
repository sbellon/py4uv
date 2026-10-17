// SPDX-FileCopyrightText: 2026 Stefan Bellon
//
// SPDX-License-Identifier: MIT

//! Every Rust source file starts with an SPDX header:
//!
//! ```text
//! // SPDX-FileCopyrightText: <year(s)> <name>
//! //
//! // SPDX-License-Identifier: MIT
//! ```
//!
//! The pre-commit hook adds it to new files and keeps the year current.

use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether `text` starts with copyright line(s), an empty comment line and
/// the MIT license identifier.
fn has_header(text: &str) -> bool {
    let mut lines = text.lines();
    let mut copyrights = 0;
    let mut line = lines.next();
    while let Some(l) = line.and_then(|l| l.strip_prefix("// SPDX-FileCopyrightText: ")) {
        let (years, name) = l.split_once(' ').unwrap_or((l, ""));
        let year_ok = years
            .split('-')
            .all(|y| y.len() == 4 && y.bytes().all(|b| b.is_ascii_digit()));
        if !year_ok || name.trim().is_empty() {
            return false;
        }
        copyrights += 1;
        line = lines.next();
    }
    copyrights > 0 && line == Some("//") && lines.next() == Some("// SPDX-License-Identifier: MIT")
}

#[test]
fn all_rust_files_have_spdx_headers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in ["src", "tests"] {
        rust_files(&root.join(dir), &mut files);
    }
    let missing: Vec<_> = files
        .iter()
        .filter(|f| !has_header(&fs::read_to_string(f).unwrap()))
        .map(|f| f.strip_prefix(root).unwrap().display().to_string())
        .collect();
    assert!(
        missing.is_empty(),
        "files without an SPDX header: {missing:?}"
    );
}

#[test]
fn header_format() {
    let ok = "// SPDX-FileCopyrightText: 2026 A B\n//\n// SPDX-License-Identifier: MIT\n";
    assert!(has_header(ok));
    assert!(has_header(&ok.replace("2026", "2024-2026")));
    assert!(has_header(&format!(
        "// SPDX-FileCopyrightText: 2025 C\n{ok}"
    )));
    assert!(!has_header("//! docs\n"));
    assert!(!has_header(&ok.replace("2026", "26")));
    assert!(!has_header(&ok.replace("MIT", "GPL-3.0")));
}
