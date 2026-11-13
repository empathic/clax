//! Golden copies for the crate's tests: text kept in the crate that a
//! test compares, byte for byte, with what the code produces now
//! (`src/api.d.ts.golden`, `src/api.web.d.ts.golden`,
//! `src/prompt.*.golden`, `tests/golden/*.json`). The unit tests have
//! this module as `crate::golden`; the integration test includes this
//! file in its `golden` module. Its own tests are `src/golden_tests.rs`,
//! which the library's unit tests alone compile.
//!
//! A golden is rewritten only on purpose: `CLAX_UPDATE_GOLDEN=1` makes
//! every comparison write the produced text over its golden instead of
//! comparing, and the golden's diff is then reviewed with the change that
//! caused it. Under Bazel that is `bazel run` of the test, which knows the
//! source tree; a `bazel test` with the variable set is refused, since its
//! copies of the goldens are not the source tree's.

use std::path::{Path, PathBuf};

/// The crate's directory: absolute under Cargo, and from the workspace
/// root under Bazel, whose tests run there.
const CRATE: &str = env!("CARGO_MANIFEST_DIR");

/// The command that rewrites the goldens of the test binary this is: of
/// the Bazel test `target` (its label, `TEST_TARGET` in a test Bazel
/// runs) when there is one, else of the Cargo package `package`.
pub(crate) fn rewrite_command(target: Option<&str>, package: &str) -> String {
    match target {
        Some(target) => format!("`CLAX_UPDATE_GOLDEN=1 bazel run {target}`"),
        None => format!("`CLAX_UPDATE_GOLDEN=1 cargo test -p {package}`"),
    }
}

/// How to rewrite the goldens this test binary compares
/// ([`rewrite_command`]).
fn how_to_rewrite() -> String {
    rewrite_command(
        std::env::var("TEST_TARGET").ok().as_deref(),
        env!("CARGO_PKG_NAME"),
    )
}

/// Where to write the golden `path` (from the crate's directory) in the
/// source tree: under the workspace a `bazel run` was started in
/// (`BUILD_WORKSPACE_DIRECTORY`), else the crate's directory as Cargo
/// gives it.
///
/// # Panics
/// Under `bazel test` (`TEST_SRCDIR` or `RUNFILES_DIR` without
/// `BUILD_WORKSPACE_DIRECTORY`), where the crate's directory is a copy.
fn source_path(path: &str) -> PathBuf {
    if let Some(workspace) = std::env::var_os("BUILD_WORKSPACE_DIRECTORY") {
        return Path::new(&workspace).join(CRATE).join(path);
    }
    let under_bazel = ["TEST_SRCDIR", "RUNFILES_DIR"]
        .iter()
        .any(|v| std::env::var_os(v).is_some());
    assert!(
        !under_bazel,
        "CLAX_UPDATE_GOLDEN is set under `bazel test`, which cannot rewrite the golden \
         {CRATE}/{path} in the source tree and would compare nothing. Rewrite it with {}.",
        how_to_rewrite()
    );
    Path::new(CRATE).join(path)
}

/// The longest differing line [`first_difference`] shows whole, in bytes.
const LONG_LINE: usize = 200;

/// The bytes [`first_difference`] shows on each side of the first
/// difference in a longer line.
const CONTEXT: usize = 80;

/// The longest text a mismatch prints whole, in bytes.
const WHOLE_TEXT: usize = 4096;

/// The part of `s` within [`CONTEXT`] bytes of byte `at`, cut at character
/// boundaries, with `…` where `s` goes on.
fn around(s: &str, at: usize) -> String {
    let mut from = at.min(s.len()).saturating_sub(CONTEXT);
    while !s.is_char_boundary(from) {
        from -= 1;
    }
    let mut to = at.saturating_add(CONTEXT).min(s.len());
    while !s.is_char_boundary(to) {
        to += 1;
    }
    format!(
        "{}{:?}{}",
        if from > 0 { "…" } else { "" },
        &s[from..to],
        if to < s.len() { "…" } else { "" }
    )
}

/// The lines before the first difference that [`first_difference`] shows
/// with it: they say where in the text the difference is.
const LINES_BEFORE: usize = 2;

/// `s` quoted, cut to its first [`CONTEXT`] bytes when it is over
/// [`LONG_LINE`] bytes.
fn quoted(s: &str) -> String {
    if s.len() > LONG_LINE {
        around(s, 0)
    } else {
        format!("{s:?}")
    }
}

/// The first line at which `want` and `got` differ, with each side's text
/// of it, or `(no such line)` for a side that ends before it, after the
/// [`LINES_BEFORE`] lines before it, which the sides share. Of a line
/// over [`LONG_LINE`] bytes on either side (a request's bytes are one
/// line), the byte at which the sides first differ (0 when one side has
/// no such line) and [`CONTEXT`] bytes of each around it.
pub(crate) fn first_difference(want: &str, got: &str) -> String {
    let (mut w, mut g) = (want.split('\n'), got.split('\n'));
    let mut line = 1;
    let mut before: Vec<&str> = Vec::new();
    loop {
        let (a, b) = (w.next(), g.next());
        if let (Some(a), true) = (a, a == b) {
            line += 1;
            before.push(a);
            continue;
        }
        let shared: String = before
            .iter()
            .skip(before.len().saturating_sub(LINES_BEFORE))
            .map(|l| format!("\n  after:  {}", quoted(l)))
            .collect();
        let len = |s: Option<&str>| s.map_or(0, str::len);
        if len(a).max(len(b)) <= LONG_LINE {
            let show =
                |s: Option<&str>| s.map_or("(no such line)".to_owned(), |s| format!("{s:?}"));
            return format!(
                "line {line}{shared}\n  golden: {}\n  now:    {}",
                show(a),
                show(b)
            );
        }
        let at = match (a, b) {
            (Some(a), Some(b)) => a
                .bytes()
                .zip(b.bytes())
                .position(|(x, y)| x != y)
                .unwrap_or(a.len().min(b.len())),
            _ => 0,
        };
        let size =
            |s: Option<&str>| s.map_or("absent".to_owned(), |s| format!("{} bytes", s.len()));
        let show = |s: Option<&str>| s.map_or("(no such line)".to_owned(), |s| around(s, at));
        return format!(
            "line {line}, byte {at} of it (the golden's line is {}, the line now {}){shared}\n  \
             golden: {}\n  now:    {}",
            size(a),
            size(b),
            show(a),
            show(b),
        );
    }
}

/// How `got`, a text of `what`, differs from the golden copy at `path`
/// (from the crate's directory), or `None` when it is that text, byte for
/// byte: where they first differ ([`first_difference`]), how to rewrite
/// the golden, and the text now when it is at most [`WHOLE_TEXT`] bytes.
/// Under `CLAX_UPDATE_GOLDEN=1` it rewrites the golden with `got` and
/// answers `None`.
///
/// # Panics
/// When the golden cannot be read, or, under `CLAX_UPDATE_GOLDEN=1`,
/// cannot be written ([`source_path`]).
pub(crate) fn mismatch(what: &str, path: &str, got: &str) -> Option<String> {
    if std::env::var_os("CLAX_UPDATE_GOLDEN").is_some() {
        let to = source_path(path);
        std::fs::write(&to, got)
            .unwrap_or_else(|e| panic!("writing the golden {}: {e}", to.display()));
        return None;
    }
    let want = std::fs::read_to_string(Path::new(CRATE).join(path)).unwrap_or_else(|e| {
        panic!(
            "reading the golden {CRATE}/{path}: {e}. A new golden is written as an existing \
             one is rewritten: {}.",
            how_to_rewrite()
        )
    });
    (got != want).then(|| {
        let whole = if got.len() <= WHOLE_TEXT {
            format!("The whole text now:\n{got}")
        } else {
            format!(
                "The whole text now is {} bytes and is left out; rewriting the golden writes it.",
                got.len()
            )
        };
        format!(
            "{what} changed: it is no longer {CRATE}/{path}, byte for byte. If the change is \
             meant, rewrite the golden with {} and review its diff; if not, undo the change.\n\
             First difference: {}\n\n{whole}",
            how_to_rewrite(),
            first_difference(&want, got),
        )
    })
}

/// Fails the test when any of `mismatches` is one ([`mismatch`]), with
/// every one of them.
///
/// # Panics
/// With every mismatch.
pub(crate) fn expect_none(mismatches: impl IntoIterator<Item = Option<String>>) {
    let found: Vec<String> = mismatches.into_iter().flatten().collect();
    assert!(
        found.is_empty(),
        "{} of the golden copies compared differ:\n\n{}",
        found.len(),
        found.join("\n\n"),
    );
}

/// Compares `got`, a text of `what`, with the golden copy at `path` (from
/// the crate's directory), or, under `CLAX_UPDATE_GOLDEN=1`, rewrites the
/// golden with `got`.
///
/// # Panics
/// When `got` is not the golden, saying where they first differ and how
/// to rewrite it.
pub(crate) fn golden(what: &str, path: &str, got: &str) {
    expect_none([mismatch(what, path, got)]);
}
