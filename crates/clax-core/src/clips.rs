//! Clips for a remote session (spec 2026-10-08 §16): the daemon names a
//! grant session's clip `clax-clip:<artifact>/<thread>` rather than a path
//! on its own machine, and the agent side fetches it, signed, into
//! `<home>/remote-cache/clips/` and rewrites the payload's `Clip:` line to
//! that file, or to `Clip: unavailable (<reason>)` when it cannot.
//!
//! A remote daemon's payload never hands the agent a path of its own: a
//! `Clip:` or `Snapshot:` line naming anything but a clip reference, a
//! placeholder or `none (…)` is rewritten to `unavailable`, so a daemon
//! cannot point the agent's file tools at a local file such as the
//! machine's key.

use crate::ids::{ArtifactId, is_ulid};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The most bytes a clip may hold, which the store accepts and a clip
/// download may read. A client reads at most one byte more, and refuses a
/// body over it.
pub const MAX_BYTES: usize = 5 * 1024 * 1024;

/// The reason a clip fetch that did not finish in time is written with.
pub const TIMED_OUT: &str = "timed out";

/// Whether a downloaded clip is one: `Content-Type: image/png` (parameters
/// ignored), at most [`MAX_BYTES`], and a PNG's signature.
///
/// # Errors
/// Why it is not, for a `Clip: unavailable (…)` line.
pub fn check(content_type: Option<&str>, bytes: &[u8]) -> Result<(), String> {
    let ct = content_type
        .and_then(|c| c.split(';').next())
        .map(|c| c.trim().to_ascii_lowercase());
    if ct.as_deref() != Some("image/png") {
        return Err("not a PNG".into());
    }
    if bytes.len() > MAX_BYTES {
        return Err(format!("over {MAX_BYTES} bytes"));
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("not a PNG".into());
    }
    Ok(())
}

/// The distinct clip references of `text`'s `Clip:` lines, in order.
pub fn references_in(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for r in text
        .lines()
        .filter_map(|l| l.strip_prefix("Clip: "))
        .filter_map(parse_reference)
    {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// One clip fetch: thread `tid`'s clip on artifact `aid`, given at most
/// the duration to finish in; the cached file, or why there is none.
pub type Fetch = dyn Fn(&str, &str, Duration) -> Result<PathBuf, String> + Send + Sync;

/// Fetches each distinct one of `refs` at once, each on a thread of its
/// own given what is left of `within`, and waits for them no longer than
/// `within`: a fetch not back by then is [`TIMED_OUT`] (its thread is left
/// to end on its own bound). So a hook that must print what it took within
/// its deadline loses no comment to a slow clip.
pub fn fetch_within(
    refs: &[(String, String)],
    within: Duration,
    fetch: std::sync::Arc<Fetch>,
) -> HashMap<(String, String), Result<PathBuf, String>> {
    let until = Instant::now() + within;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut out = HashMap::new();
    let mut waiting = 0;
    for r in refs {
        if out.contains_key(r) {
            continue;
        }
        out.insert(r.clone(), Err(TIMED_OUT.to_string()));
        let (tx, fetch, r) = (tx.clone(), fetch.clone(), r.clone());
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            continue;
        }
        waiting += 1;
        std::thread::spawn(move || {
            let got = fetch(&r.0, &r.1, left);
            let _ = tx.send((r, got));
        });
    }
    while waiting > 0 {
        let left = until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok((r, got)) => {
                out.insert(r, got);
                waiting -= 1;
            }
            Err(_) => break,
        }
    }
    out
}

/// The scheme of a clip reference.
pub const SCHEME: &str = "clax-clip:";

/// The reference to thread `tid`'s clip on artifact `aid`.
pub fn reference(aid: &str, tid: &str) -> String {
    format!("{SCHEME}{aid}/{tid}")
}

/// The artifact and thread a clip reference names, when `s` is one with an
/// artifact ID and a ULID thread ID (nothing else, so neither can step out
/// of the cache directory).
pub fn parse_reference(s: &str) -> Option<(String, String)> {
    let (aid, tid) = s.strip_prefix(SCHEME)?.split_once('/')?;
    (ArtifactId::parse(aid).is_ok() && is_ulid(tid)).then(|| (aid.to_string(), tid.to_string()))
}

/// Where thread `tid`'s clip on artifact `aid` is cached under `dir`
/// (`<home>/remote-cache/clips`).
pub fn cache_path(dir: &Path, aid: &str, tid: &str) -> PathBuf {
    dir.join(format!("{aid}-{tid}.png"))
}

/// The line for a remote clip that could not be fetched.
pub fn unavailable(reason: &str) -> String {
    format!("Clip: unavailable ({})", crate::feedback::one_line(reason))
}

/// What a remote daemon's `Clip:` value becomes: `None` to keep it (a
/// placeholder or `none (…)`), else the replacement value.
fn rewrite_value(
    value: &str,
    fetch: &mut dyn FnMut(&str, &str) -> Result<PathBuf, String>,
) -> Option<String> {
    if value == "withheld" || value.starts_with("none (") {
        return None;
    }
    Some(match parse_reference(value) {
        Some((aid, tid)) => match fetch(&aid, &tid) {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(why) => format!("unavailable ({})", crate::feedback::one_line(&why)),
        },
        None => "unavailable (the remote daemon named no clip)".to_string(),
    })
}

/// `text`, a remote daemon's payload, with every `Clip:` line rewritten
/// (see the module comment): a reference to the file `fetch` cached for
/// it, or `unavailable (<why>)` when it failed; `Clip: withheld` and `Clip:
/// none (…)` are kept without a fetch. A `Snapshot:` line (a live page's,
/// a path on the daemon's machine) becomes `Snapshot: unavailable (…)`.
pub fn rewrite_text(
    text: &str,
    fetch: &mut dyn FnMut(&str, &str) -> Result<PathBuf, String>,
) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if let Some(value) = line.strip_prefix("Clip: ") {
            match rewrite_value(value, fetch) {
                Some(v) => {
                    out.push_str("Clip: ");
                    out.push_str(&v);
                }
                None => out.push_str(line),
            }
        } else if line.starts_with("Snapshot: ") {
            out.push_str("Snapshot: unavailable (on the remote daemon's machine)");
        } else {
            out.push_str(line);
        }
    }
    out
}

/// A remote daemon's feedback item or thread view (`v`, a JSON object)
/// with its `clip_path` rewritten as [`rewrite_text`] rewrites a line: the
/// cached file, or null (with `clip_unavailable` saying why); and any
/// `snapshot_path` made null.
pub fn rewrite_json(
    v: &mut serde_json::Value,
    fetch: &mut dyn FnMut(&str, &str) -> Result<PathBuf, String>,
) {
    let Some(o) = v.as_object_mut() else { return };
    if let Some(p) = o
        .get("clip_path")
        .and_then(|p| p.as_str())
        .map(str::to_string)
    {
        match parse_reference(&p).map(|(aid, tid)| fetch(&aid, &tid)) {
            Some(Ok(path)) => {
                o.insert(
                    "clip_path".into(),
                    serde_json::json!(path.to_string_lossy()),
                );
            }
            Some(Err(why)) => {
                o.insert("clip_path".into(), serde_json::Value::Null);
                o.insert("clip_unavailable".into(), serde_json::json!(why));
            }
            None => {
                o.insert("clip_path".into(), serde_json::Value::Null);
            }
        }
    }
    if o.get("snapshot_path").is_some_and(|p| !p.is_null()) {
        o.insert("snapshot_path".into(), serde_json::Value::Null);
    }
}

/// Writes clip `bytes` to the cache file of `(aid, tid)` under `dir`
/// (0700, files 0600, written whole), answering its path.
///
/// # Errors
/// The I/O error.
pub fn store(dir: &Path, aid: &str, tid: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    if meta.permissions().mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = cache_path(dir, aid, tid);
    crate::remote::write_private(&path, bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AID: &str = "7q3k9mzx2b4t";
    const TID: &str = "01J9ZZZZZZZZZZZZZZZZZZZZZZ";

    #[test]
    fn references_name_an_artifact_and_a_thread_only() {
        let r = reference(AID, TID);
        assert_eq!(r, format!("clax-clip:{AID}/{TID}"));
        assert_eq!(parse_reference(&r), Some((AID.into(), TID.into())));
        for bad in [
            format!("clax-clip:../x/{TID}"),
            format!("clax-clip:{AID}/../../remote.key"),
            format!("clax-clip:{AID}/{TID}/x"),
            "/Users/sam/.clax/remote.key".to_string(),
            format!("{AID}/{TID}"),
        ] {
            assert_eq!(parse_reference(&bad), None, "{bad}");
        }
    }

    #[test]
    fn lines_are_rewritten_to_the_fetched_file_or_unavailable() {
        let text = format!(
            "[clax] Comment sent to you on \"Doc\"\nAnchored on: h2\nClip: clax-clip:{AID}/{TID}\nMia: hi\n\
             ---\nClip: withheld\nClip: none (the comment is about the whole artifact)\n\
             Clip: /Users/sam/.clax/remote.key\nSnapshot: /Users/alex/.clax/live/x.png"
        );
        let mut asked = Vec::new();
        let out = rewrite_text(&text, &mut |a, t| {
            asked.push((a.to_string(), t.to_string()));
            Ok(PathBuf::from("/c/x.png"))
        });
        assert_eq!(asked, [(AID.to_string(), TID.to_string())], "one fetch");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[2], "Clip: /c/x.png");
        assert_eq!(lines[5], "Clip: withheld");
        assert_eq!(
            lines[6],
            "Clip: none (the comment is about the whole artifact)"
        );
        assert_eq!(
            lines[7],
            "Clip: unavailable (the remote daemon named no clip)"
        );
        assert_eq!(
            lines[8],
            "Snapshot: unavailable (on the remote daemon's machine)"
        );
        let out = rewrite_text(&text, &mut |_, _| Err("HTTP 404".into()));
        assert!(out.contains("\nClip: unavailable (HTTP 404)\n"), "{out}");
    }

    #[test]
    fn json_items_carry_the_fetched_file_and_never_a_remote_path() {
        let mut v = serde_json::json!({"clip_path": reference(AID, TID), "snapshot_path": "/x"});
        rewrite_json(&mut v, &mut |_, _| Ok(PathBuf::from("/c/y.png")));
        assert_eq!(v["clip_path"], "/c/y.png");
        assert!(v["snapshot_path"].is_null());
        let mut v = serde_json::json!({"clip_path": "/Users/sam/.ssh/id_ed25519"});
        rewrite_json(&mut v, &mut |_, _| panic!("no fetch for a path"));
        assert!(v["clip_path"].is_null());
        let mut v = serde_json::json!({"clip_path": reference(AID, TID)});
        rewrite_json(&mut v, &mut |_, _| Err("withheld".into()));
        assert!(v["clip_path"].is_null());
        assert_eq!(v["clip_unavailable"], "withheld");
    }

    #[test]
    fn a_clip_is_checked_before_it_is_kept() {
        let png = b"\x89PNG\r\n\x1a\nrest";
        assert_eq!(check(Some("image/png"), png), Ok(()));
        assert_eq!(check(Some("Image/PNG; x=1"), png), Ok(()));
        assert!(check(Some("text/html"), png).is_err());
        assert!(check(None, png).is_err());
        assert!(check(Some("image/png"), b"<html>").is_err());
        let mut big = png.to_vec();
        big.resize(MAX_BYTES + 1, 0);
        assert!(
            check(Some("image/png"), &big)
                .unwrap_err()
                .starts_with("over")
        );
    }

    /// I1: references are fetched once each, at once; a fetch not back
    /// within the bound is `timed out`, and the others are kept.
    #[test]
    fn fetches_are_deduplicated_concurrent_and_bounded() {
        use std::sync::{Arc, Mutex};
        let slow = "01J9ZZZZZZZZZZZZZZZZZZZZZY".to_string();
        let calls = Arc::new(Mutex::new(Vec::new()));
        // The slow fetch waits on this until the test has its answer.
        let (release, held) = std::sync::mpsc::channel::<()>();
        let held = Mutex::new(held);
        let seen = calls.clone();
        let slow_tid = slow.clone();
        let fetch: Arc<Fetch> = Arc::new(move |a: &str, t: &str, left: Duration| {
            seen.lock().unwrap().push((t.to_string(), left));
            if t == slow_tid {
                let _ = held.lock().unwrap().recv();
                return Err("late".into());
            }
            Ok(PathBuf::from(format!("/c/{a}-{t}.png")))
        });
        let text = format!(
            "Clip: clax-clip:{AID}/{TID}\nClip: clax-clip:{AID}/{TID}\nClip: clax-clip:{AID}/{slow}"
        );
        let refs = references_in(&text);
        assert_eq!(refs.len(), 2, "deduplicated");
        let bound = Duration::from_secs(1);
        let got = fetch_within(&refs, bound, fetch);
        let _ = release.send(());
        assert_eq!(
            got[&(AID.to_string(), TID.to_string())],
            Ok(PathBuf::from(format!("/c/{AID}-{TID}.png")))
        );
        assert_eq!(
            got[&(AID.to_string(), slow.clone())],
            Err(TIMED_OUT.to_string())
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|(_, left)| *left <= bound));
        let out = rewrite_text(&text, &mut |a, t| {
            got[&(a.to_string(), t.to_string())].clone()
        });
        assert!(out.ends_with("Clip: unavailable (timed out)"), "{out}");
        // Nothing is fetched once the bound is spent.
        let none: Arc<Fetch> = Arc::new(|_: &str, _: &str, _: Duration| panic!("no fetch"));
        let got = fetch_within(&refs, Duration::ZERO, none);
        assert!(got.values().all(|r| r == &Err(TIMED_OUT.to_string())));
    }

    #[test]
    fn the_cache_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("remote-cache/clips");
        let p = store(&dir, AID, TID, b"png").unwrap();
        assert_eq!(p, cache_path(&dir, AID, TID));
        assert_eq!(std::fs::read(&p).unwrap(), b"png");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir), mode(&p)), (0o700, 0o600));
    }
}
