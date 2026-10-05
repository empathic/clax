//! Stand-in executables for tests: scripts a test puts where the code under
//! test looks for a program (`codex`, `clax`, an opener, a daemon).
//!
//! macOS assesses every new executable file the first time it runs
//! (syspolicyd, XProtect). On a loaded machine that first run can take
//! seconds, longer than the limit the code under test puts on a program it
//! starts, while later runs of the same file take milliseconds. The
//! assessment belongs to the file, not to its text or its name: a new file
//! with the same text is assessed again; a hard link to an assessed file is
//! not.
//!
//! So [`install`] keeps one shared, read-only copy of each script text in
//! the system's temporary directory, named by a hash of the text, runs it
//! once (with `CLAX_FAKE_EXE_WARMUP` set, which makes it exit at once)
//! before any test relies on it, and hard-links it where the test asks. A
//! test's own runs of the script are then never a first run. Per-test state
//! therefore stays out of the text: a script finds its test's files beside
//! itself (`$(dirname "$0")`), in its arguments or in its environment.

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
/// returns `at`. The file is a hard link to the shared copy and is
/// read-only: to change a script, install another text at the same path.
///
/// Panics when the script cannot be stored or its warm-up run fails.
pub fn install(at: &Path, text: &str) -> PathBuf {
    let shared = shared_copy(text);
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("creating {parent:?}: {e}"));
    }
    match std::fs::remove_file(at) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => panic!("removing {at:?}: {e}"),
    }
    if std::fs::hard_link(&shared, at).is_err() {
        // Another file system: a copy of its own, warmed here.
        std::fs::copy(&shared, at).unwrap_or_else(|e| panic!("copying {shared:?} to {at:?}: {e}"));
        warm(at);
    }
    at.to_path_buf()
}

/// The shared copy of `text`, with the warm-up guard, created and run once
/// by this process before it is returned.
fn shared_copy(text: &str) -> PathBuf {
    static WARMED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
    let text = with_guard(text);
    let dir = std::env::temp_dir().join("clax-fake-exe");
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
        // Written, made read-only and run under a name of its own, then
        // renamed into place: concurrent tests storing the same text each
        // rename a warmed file, and a reader never sees a partial one.
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp = dir.join(format!(
            ".{name}.{}.{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&tmp, &text).unwrap_or_else(|e| panic!("writing {tmp:?}: {e}"));
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o555))
            .unwrap_or_else(|e| panic!("making {tmp:?} executable: {e}"));
        warm(&tmp);
        std::fs::rename(&tmp, &shared)
            .unwrap_or_else(|e| panic!("renaming {tmp:?} to {shared:?}: {e}"));
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
            let p = install(&t.path().join("tool"), text);
            assert!(!mark.exists(), "the warm-up ran only the guard");
            assert!(Command::new(&p).status().unwrap().success());
            assert!(mark.exists());
            std::fs::remove_file(&mark).unwrap();
        }
    }
}
