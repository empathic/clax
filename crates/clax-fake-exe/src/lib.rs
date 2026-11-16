//! Stand-in executables for tests: scripts a test puts where the code under
//! test looks for a program (`codex`, `clax`, an opener, a daemon).
//!
//! macOS assesses every new executable file the first time it runs
//! (syspolicyd, XProtect). On a loaded machine that first run can take
//! seconds, longer than the limit the code under test puts on a program it
//! starts, while later runs of the same file take milliseconds. The
//! assessment belongs to the file, not to its text or its name: a new file
//! with the same text is assessed again; running an assessed file through a
//! symbolic link to it is not a first run.
//!
//! So [`install`] keeps one shared, read-only copy of each script text in
//! the system's temporary directory, named by a hash of the text, runs it
//! once (with `CLAX_FAKE_EXE_WARMUP` set, which makes it exit at once)
//! before any test relies on it, and puts a symbolic link to it where the
//! test asks. A test's own runs of the script are then never a first run.
//! Per-test state therefore stays out of the text: a script finds its test's
//! files beside itself (`$(dirname "$0")`, which names the link's directory),
//! in its arguments or in its environment.
//!
//! A path resolved through symbolic links (`realpath`, `canonicalize`) names
//! the shared copy, so scripts with the same text resolve to one file, and
//! installing another text at a path changes what it resolves to rather
//! than the file there. Where the code under test identifies a program by
//! its file (its resolved path, its modification time), as it does an
//! installed `clax`, [`install_own`] puts a file of its own at the path
//! instead, run once there before it is returned: a first run per call,
//! paid outside any limit the test sets.

pub mod bin;
pub use bin::{clax_bin, repo};

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// The variable that makes an installed script exit at once, for its
/// warm-up run.
pub const WARMUP_VAR: &str = "CLAX_FAKE_EXE_WARMUP";

/// Puts an executable script with `text` (which starts with a `#!` line
/// naming a shell or Python) at `at`, replacing whatever is there, and
/// returns `at`. `at` is a symbolic link to the shared copy, which is
/// read-only: to change a script, install another text at the same path.
///
/// Panics when the script cannot be stored or its warm-up run fails.
pub fn install(at: &Path, text: &str) -> PathBuf {
    let shared = shared_copy(text);
    clear(at);
    std::os::unix::fs::symlink(&shared, at)
        .unwrap_or_else(|e| panic!("linking {at:?} to {shared:?}: {e}"));
    at.to_path_buf()
}

/// Like [`install`], but `at` is a read-only file of its own, already run
/// once: for a program the code under test identifies by its file.
/// Installing at the same path again replaces the file, as an in-place
/// reinstall does (same path, new modification time).
///
/// Panics when the script cannot be written or its warm-up run fails.
pub fn install_own(at: &Path, text: &str) -> PathBuf {
    clear(at);
    let dir = at.parent().unwrap_or(Path::new("."));
    let name = at
        .file_name()
        .map_or("fake".into(), |n| n.to_string_lossy());
    store(
        &dir.join(format!(".{name}.{}", unique())),
        &with_guard(text),
        at,
    );
    at.to_path_buf()
}

/// Makes `at`'s directory and removes whatever is at `at`.
fn clear(at: &Path) {
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("creating {parent:?}: {e}"));
    }
    match std::fs::remove_file(at) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => panic!("removing {at:?}: {e}"),
    }
}

/// A suffix no other call in any process uses at the same time.
fn unique() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Writes `text` to `tmp`, makes it read-only, runs it once and renames it
/// to `to`, so whoever finds a file at `to` finds a whole, warmed one. A
/// rename keeps the file, so its first run stays behind it.
fn store(tmp: &Path, text: &str, to: &Path) {
    std::fs::write(tmp, text).unwrap_or_else(|e| panic!("writing {tmp:?}: {e}"));
    std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o555))
        .unwrap_or_else(|e| panic!("making {tmp:?} executable: {e}"));
    warm(tmp);
    std::fs::rename(tmp, to).unwrap_or_else(|e| panic!("renaming {tmp:?} to {to:?}: {e}"));
}

/// `path` (a `PATH` value) with each program in `hide` absent. Each
/// directory that holds one of them is replaced by a directory under `out`
/// with symbolic links to its other entries. A directory that resolves to
/// one already seen (`/bin` linked to `/usr/bin`) is dropped. A later call
/// with the same `out`, directory and `hide` reuses the directory it made.
///
/// Panics when `out` cannot be written.
pub fn path_without(path: &str, out: &Path, hide: &[&str]) -> std::ffi::OsString {
    let mut seen = HashSet::new();
    let mut dirs = Vec::new();
    for dir in std::env::split_paths(path) {
        if !seen.insert(std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())) {
            continue;
        }
        if !hide.iter().any(|h| dir.join(h).symlink_metadata().is_ok()) {
            dirs.push(dir);
            continue;
        }
        let key = format!("{}\0{}", dir.display(), hide.join("\0"));
        let shadow = out.join(format!("{:016x}", fnv1a(key.as_bytes())));
        if !shadow.is_dir() {
            let tmp = out.join(format!(".{}", unique()));
            std::fs::create_dir_all(&tmp).unwrap_or_else(|e| panic!("creating {tmp:?}: {e}"));
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let name = entry.file_name();
                if hide.iter().any(|h| name == *h) {
                    continue;
                }
                let at = tmp.join(&name);
                std::os::unix::fs::symlink(entry.path(), &at)
                    .unwrap_or_else(|e| panic!("linking {at:?}: {e}"));
            }
            // Another thread can win the rename; its directory is the same.
            if std::fs::rename(&tmp, &shadow).is_err() {
                let _ = std::fs::remove_dir_all(&tmp);
                assert!(shadow.is_dir(), "renaming {tmp:?} to {shadow:?}");
            }
        }
        dirs.push(shadow);
    }
    std::env::join_paths(dirs).expect("PATH entries hold no ':'")
}

/// The store for shared copies: `CLAX_FAKE_EXE_DIR` when set and not empty
/// (Bazel's tests set it, so the store outlives one test's temporary
/// directory), else `<tmp>/clax-fake-exe`.
pub fn store_dir_from(var: Option<std::ffi::OsString>, tmp: &Path) -> PathBuf {
    match var {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => tmp.join("clax-fake-exe"),
    }
}

/// The shared copy of `text`, with the warm-up guard, created and run once
/// by this process before it is returned.
fn shared_copy(text: &str) -> PathBuf {
    static WARMED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
    let text = with_guard(text);
    let dir = store_dir_from(std::env::var_os("CLAX_FAKE_EXE_DIR"), &std::env::temp_dir());
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("creating {dir:?}: {e}"));
    let name = format!("{:016x}-{}", fnv1a(text.as_bytes()), text.len());
    let shared = dir.join(&name);
    let mut warmed = WARMED.lock().unwrap_or_else(|e| e.into_inner());
    let warmed = warmed.get_or_insert_with(HashSet::new);
    if warmed.contains(&shared) {
        return shared;
    }
    if shared.exists() {
        // Assessed when it was stored; run again in case that was before a
        // restart, and to check it is whole.
        warm(&shared);
    } else {
        // Stored under a name of its own, then renamed into place:
        // concurrent tests storing the same text each rename a warmed file.
        store(&dir.join(format!(".{name}.{}", unique())), &text, &shared);
    }
    warmed.insert(shared.clone());
    shared
}

/// `text` with a line after its `#!` line that ends a warm-up run.
fn with_guard(text: &str) -> String {
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    assert!(
        first.starts_with("#!"),
        "a fake executable starts with a #! line: {first:?}"
    );
    let guard = if first.contains("python") {
        format!("import os as _o, sys as _s\nif _o.environ.get(\"{WARMUP_VAR}\"): _s.exit(0)")
    } else {
        format!("[ -z \"${{{WARMUP_VAR}:-}}\" ] || exit 0")
    };
    format!("{first}\n{guard}\n{rest}")
}

/// Runs the script at `path` once, as a warm-up, waiting however long its
/// first run takes. On Linux a file just written can briefly be "busy" to
/// exec while a forked child of another thread still holds its write
/// descriptor; that is waited out.
fn warm(path: &Path) {
    let mut busy = 0;
    loop {
        let r = Command::new(path)
            .env(WARMUP_VAR, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match r {
            Ok(s) if s.success() => return,
            Ok(s) => panic!("the warm-up run of {path:?} exited {s}"),
            // ETXTBSY
            Err(e) if e.raw_os_error() == Some(26) && busy < 1000 => {
                busy += 1;
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(e) => panic!("running {path:?}: {e}"),
        }
    }
}

/// 64-bit FNV-1a: a stable name for a text.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_without_hides_only_the_named_programs() {
        let d = tempfile::tempdir().unwrap();
        let sys = d.path().join("sys");
        std::fs::create_dir_all(&sys).unwrap();
        let alias = d.path().join("alias");
        std::os::unix::fs::symlink(&sys, &alias).unwrap();
        let own = d.path().join("own");
        install(&sys.join("codex"), "#!/bin/sh\nexit 1\n");
        install(&sys.join("sh2"), "#!/bin/sh\nexit 0\n");
        install(&own.join("codex"), "#!/bin/sh\nexit 0\n");
        let base = format!("{}:{}", sys.display(), alias.display());
        let path = path_without(&base, &d.path().join("out"), &["codex"]);
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].starts_with(d.path().join("out")));
        assert!(dirs[0].join("sh2").exists());
        assert!(!dirs[0].join("codex").exists());
        assert_eq!(path_without(&base, &d.path().join("out"), &["codex"]), path);
        let mine = format!("{}:{base}", own.display());
        let path = path_without(&mine, &d.path().join("out2"), &["claude"]);
        assert_eq!(std::env::split_paths(&path).collect::<Vec<_>>(), [own, sys]);
    }

    #[test]
    fn the_store_is_clax_fake_exe_dir_when_set() {
        assert_eq!(
            store_dir_from(Some("/s/fakes".into()), Path::new("/tmp")),
            PathBuf::from("/s/fakes")
        );
        assert_eq!(
            store_dir_from(None, Path::new("/tmp")),
            PathBuf::from("/tmp/clax-fake-exe")
        );
        assert_eq!(
            store_dir_from(Some("".into()), Path::new("/tmp")),
            PathBuf::from("/tmp/clax-fake-exe")
        );
    }

    #[test]
    fn installs_a_runnable_link_to_one_shared_copy() {
        let t = tempfile::tempdir().unwrap();
        let text = "#!/bin/sh\nprintf '%s|%s' \"$(basename \"$(dirname \"$0\")\")\" \"$*\"\n";
        let a = install(&t.path().join("a/tool"), text);
        let b = install(&t.path().join("b/tool"), text);
        let run = |p: &Path| {
            let o = Command::new(p).arg("x").output().unwrap();
            String::from_utf8(o.stdout).unwrap()
        };
        assert_eq!(run(&a), "a|x", "the script sees its own path");
        assert_eq!(run(&b), "b|x");
        use std::os::unix::fs::MetadataExt;
        assert!(std::fs::symlink_metadata(&a).unwrap().is_symlink());
        assert_eq!(
            std::fs::metadata(&a).unwrap().ino(),
            std::fs::metadata(&b).unwrap().ino(),
            "one file"
        );
        assert!(std::fs::write(&a, "x").is_err(), "read-only");
        let c = install(&a, "#!/bin/sh\necho other\n");
        assert_eq!(run(&c), "other\n", "replaced");
        assert_eq!(run(&b), "b|x", "the other link is unchanged");
    }

    #[test]
    fn a_warm_up_run_does_nothing() {
        let t = tempfile::tempdir().unwrap();
        let mark = t.path().join("mark");
        for text in [
            "#!/bin/sh\ntouch \"$(dirname \"$0\")/mark\"\n",
            "#!/usr/bin/env python3\nimport os, sys\nopen(os.path.join(os.path.dirname(sys.argv[0]), 'mark'), 'w').close()\n",
        ] {
            for put in [install, install_own] {
                let p = put(&t.path().join("tool"), text);
                assert!(!mark.exists(), "the warm-up ran only the guard");
                assert!(Command::new(&p).status().unwrap().success());
                assert!(mark.exists());
                std::fs::remove_file(&mark).unwrap();
            }
        }
    }

    #[test]
    fn installs_a_file_of_its_own_where_asked() {
        use std::os::unix::fs::MetadataExt;
        let t = tempfile::tempdir().unwrap();
        let text = "#!/bin/sh\necho one\n";
        let a = install_own(&t.path().join("a/tool"), text);
        let b = install_own(&t.path().join("b/tool"), text);
        let meta = |p: &Path| std::fs::symlink_metadata(p).unwrap();
        assert!(meta(&a).is_file(), "not a link");
        assert_ne!(meta(&a).ino(), meta(&b).ino(), "a file each");
        assert_eq!(
            a.canonicalize().unwrap().parent(),
            Some(t.path().join("a").canonicalize().unwrap().as_path())
        );
        assert!(std::fs::write(&a, "x").is_err(), "read-only");
        let before = meta(&a).ino();
        // Held open, the old file keeps its inode, so the new file cannot
        // reuse it (Linux hands a freed inode straight back out).
        let held = std::fs::File::open(&a).unwrap();
        let a2 = install_own(&a, "#!/bin/sh\necho two\n");
        assert_ne!(meta(&a2).ino(), before, "replaced by a new file");
        drop(held);
        let o = Command::new(&a2).output().unwrap();
        assert_eq!(String::from_utf8(o.stdout).unwrap(), "two\n");
        let names: Vec<_> = std::fs::read_dir(t.path().join("a"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["tool"], "no temporary file left");
    }
}
