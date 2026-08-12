//! Guards contract expectations against platform-specific path construction.

#![allow(
    clippy::expect_used,
    reason = "Unreadable checked-in test sources are unrecoverable guard harness failures."
)]

use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).expect("read test source directory") {
        let entry = entry.expect("read test source entry");
        let path = entry.path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files
}

fn hardcoded_path_separator(line: &str) -> bool {
    let mut characters = line.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character != '}' {
            continue;
        }
        let Some((_, separator)) = characters.peek() else {
            continue;
        };
        let interpolation = line[..index]
            .rsplit_once('{')
            .map_or("", |(_, interpolation)| interpolation);
        let path_context = [
            "path",
            "dest",
            "home",
            "root",
            "directory",
            "location",
            "deployment",
            "copied",
            "overwritten",
        ]
        .iter()
        .any(|needle| interpolation.contains(needle));
        if !path_context {
            continue;
        }
        if *separator == '/' {
            return true;
        }
        if *separator == '\\'
            && characters
                .clone()
                .nth(1)
                .is_some_and(|(_, next)| next == '\\')
        {
            return true;
        }
    }
    false
}

fn test_regions<'a>(path: &Path, source: &'a str) -> Vec<(usize, &'a str)> {
    if path.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")) {
        return vec![(1, source)];
    }

    source
        .match_indices("#[cfg(test)]")
        .map(|(offset, _)| {
            let line = source[..offset]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            (line, &source[offset..])
        })
        .collect()
}

#[test]
fn expected_paths_use_path_join_not_hardcoded_separators() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = rust_files(&crate_root.join("tests"));
    sources.extend(rust_files(&crate_root.join("src")));

    let mut violations = Vec::new();
    for path in sources {
        let source = fs::read_to_string(&path).expect("read Rust test source");
        for (start_line, region) in test_regions(&path, &source) {
            for (offset, line) in region.lines().enumerate() {
                if hardcoded_path_separator(line) {
                    let relative = path
                        .strip_prefix(crate_root)
                        .expect("test source is under crate root");
                    violations.push(format!("{}:{}", relative.display(), start_line + offset));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "hardcoded path separator adjacent to a format interpolation in an expected path: \
         {}. Build expected paths with Path::join(...), then use .display() or serialize it.",
        violations.join(", ")
    );
}
