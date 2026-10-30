//! Thread changes as chitin topic events: on a local daemon whose
//! environment names a chitin title (`CLAX_CHITIN_AS`), each new thread,
//! reply and resolve is published with `chitin publish --as <title>` on
//! topic `clax.thread.<artifact id>`, so a seat that watches the artifact
//! wakes from chitin instead of polling `clax comments`.
//!
//! - **Content-free.** An event names the artifact (in the topic), the
//!   thread (`t-<thread id>`, the subject) and, for a comment, 24 hex digits
//!   of the SHA-256 of the comment's ID (`c:<hex>`, the dataref). No body,
//!   author or anchor leaves the daemon: a woken seat reads the thread over
//!   the REST contract, as its rights allow.
//! - **Types.** `comment` for a thread's first comment (the first `thread`
//!   event this process sees for a thread with one comment: after a restart
//!   a later change to such a thread is announced again, a spare doorbell),
//!   `reply` for each later comment, `resolved` when it is resolved.
//! - **Off by default.** Unset or empty `CLAX_CHITIN_AS`, a hosted daemon,
//!   or no `chitin` (`CLAX_CHITIN_BIN`, else on `PATH`) publishes nothing.
//!   A publish that fails or outlasts [`PUBLISH_TIMEOUT`] is logged and
//!   dropped: chitin is a doorbell, the thread stays in the store.

use clax_core::Event;
use clax_http::state::AppState;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// Deadline for one `chitin publish` run.
pub const PUBLISH_TIMEOUT: Duration = Duration::from_secs(5);

/// Who publishes, and with which `chitin`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Publisher {
    pub bin: PathBuf,
    /// The chitin title signing each event (a declared daemon, e.g. `clax`).
    pub title: String,
}

impl Publisher {
    /// From `CLAX_CHITIN_AS` (the title; unset or empty: `None`) and
    /// `CLAX_CHITIN_BIN` (the binary, else `chitin` on `path`).
    pub fn from_env(
        title: Option<OsString>,
        bin: Option<OsString>,
        path: Option<&std::ffi::OsStr>,
    ) -> Option<Publisher> {
        let title = title?.into_string().ok().filter(|t| !t.is_empty())?;
        let bin = match bin.filter(|b| !b.is_empty()) {
            Some(b) => PathBuf::from(b),
            None => clax_realtime::push::find_on_path("chitin", path)?,
        };
        Some(Publisher { bin, title })
    }
}

/// One topic event: `chitin publish --dataref <dataref> <topic> <kind> <subject>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doorbell {
    pub topic: String,
    pub kind: &'static str,
    pub subject: String,
    pub dataref: Option<String>,
}

/// `c:` and the first 24 hex digits of the SHA-256 of comment ID `id`.
fn comment_ref(id: &str) -> String {
    let d = Sha256::digest(id.as_bytes());
    let hex: String = d.iter().take(12).map(|b| format!("{b:02x}")).collect();
    format!("c:{hex}")
}

fn topic(aid: &str) -> String {
    format!("clax.thread.{aid}")
}

/// The doorbell event `ev` rings, if any; `announced` holds the threads
/// whose first comment this process has published.
pub fn doorbell(ev: &Event, announced: &mut HashSet<String>) -> Option<Doorbell> {
    match ev {
        Event::Thread {
            artifact_id,
            thread,
        } => {
            let tid = thread["id"].as_str()?;
            let comments = thread["comments"].as_array()?;
            if comments.len() != 1 || !announced.insert(tid.to_string()) {
                return None;
            }
            Some(Doorbell {
                topic: topic(artifact_id),
                kind: "comment",
                subject: format!("t-{tid}"),
                dataref: comments[0]["id"].as_str().map(comment_ref),
            })
        }
        Event::Comment {
            artifact_id,
            thread_id,
            comment,
        } => {
            // Its thread is past its first comment: a `thread` event later
            // with one comment is not news.
            announced.insert(thread_id.clone());
            Some(Doorbell {
                topic: topic(artifact_id),
                kind: "reply",
                subject: format!("t-{thread_id}"),
                dataref: comment["id"].as_str().map(comment_ref),
            })
        }
        Event::ThreadResolved {
            artifact_id,
            thread_id,
            ..
        } => Some(Doorbell {
            topic: topic(artifact_id),
            kind: "resolved",
            subject: format!("t-{thread_id}"),
            dataref: None,
        }),
        _ => None,
    }
}

/// Runs `chitin publish` for `d`, stdio detached, killed after `timeout`.
/// `Err` says why it was not published.
pub async fn publish(p: &Publisher, d: &Doorbell, timeout: Duration) -> Result<(), String> {
    publish_with(&p.bin, &p.title, d, timeout).await
}

async fn publish_with(
    bin: &Path,
    title: &str,
    d: &Doorbell,
    timeout: Duration,
) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(["publish", "--as", title]);
    if let Some(r) = &d.dataref {
        cmd.args(["--dataref", r]);
    }
    cmd.args([d.topic.as_str(), d.kind, d.subject.as_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(s)) if s.success() => Ok(()),
        Ok(Ok(s)) => Err(format!("chitin publish exited with {s}")),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => {
            let _ = child.kill().await;
            Err("chitin publish timed out".to_string())
        }
    }
}

/// Starts the publisher on a local daemon whose environment names a chitin
/// title (see the module); returns its task's handle, or none.
pub fn listen(s: &AppState) -> Vec<clax_rt::JoinHandle<()>> {
    if s.hosted.is_some() {
        return Vec::new();
    }
    let Some(p) = Publisher::from_env(
        std::env::var_os("CLAX_CHITIN_AS"),
        std::env::var_os("CLAX_CHITIN_BIN"),
        std::env::var_os("PATH").as_deref(),
    ) else {
        return Vec::new();
    };
    tracing::info!(title = %p.title, bin = %p.bin.display(), "publishing thread changes to chitin");
    let mut rx = s.events.subscribe();
    vec![clax_rt::spawn(async move {
        let mut announced = HashSet::new();
        loop {
            let ev = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(missed = n, "chitin publisher lagged; events dropped");
                    continue;
                }
                Err(_) => return,
            };
            let Some(d) = doorbell(&ev.event, &mut announced) else {
                continue;
            };
            if let Err(e) = publish(&p, &d, PUBLISH_TIMEOUT).await {
                tracing::warn!(topic = %d.topic, kind = d.kind, error = %e, "chitin publish failed");
            }
        }
    })]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn thread(aid: &str, tid: &str, comments: serde_json::Value) -> Event {
        Event::Thread {
            artifact_id: aid.into(),
            thread: json!({"id": tid, "comments": comments}),
        }
    }

    #[test]
    fn a_new_thread_rings_comment_once_with_its_first_comments_hash() {
        let mut seen = HashSet::new();
        let ev = thread("a1", "t1", json!([{"id": "c1", "body": "secret"}]));
        let d = doorbell(&ev, &mut seen).expect("a doorbell");
        assert_eq!(
            d,
            Doorbell {
                topic: "clax.thread.a1".into(),
                kind: "comment",
                subject: "t-t1".into(),
                dataref: Some(comment_ref("c1")),
            }
        );
        assert_eq!(d.dataref.as_deref().map(str::len), Some(2 + 24));
        assert_eq!(
            doorbell(&ev, &mut seen),
            None,
            "the same thread changing again"
        );
    }

    #[test]
    fn a_reply_rings_reply_and_quiets_its_threads_later_upserts() {
        let mut seen = HashSet::new();
        let ev = Event::Comment {
            artifact_id: "a1".into(),
            thread_id: "t1".into(),
            comment: json!({"id": "c2", "body": "secret"}),
        };
        let d = doorbell(&ev, &mut seen).expect("a doorbell");
        assert_eq!((d.kind, d.subject.as_str()), ("reply", "t-t1"));
        assert_eq!(d.dataref, Some(comment_ref("c2")));
        assert_eq!(
            doorbell(&thread("a1", "t1", json!([{"id": "c1"}])), &mut seen),
            None
        );
    }

    #[test]
    fn a_resolve_rings_resolved_and_other_events_ring_nothing() {
        let mut seen = HashSet::new();
        let ev = Event::ThreadResolved {
            artifact_id: "a1".into(),
            thread_id: "t1".into(),
            resolved_by: "owner".into(),
            resolved_at: "t".into(),
        };
        let d = doorbell(&ev, &mut seen).expect("a doorbell");
        assert_eq!((d.kind, d.dataref), ("resolved", None));
        assert_eq!(
            doorbell(
                &thread("a1", "t2", json!([{"id": "a"}, {"id": "b"}])),
                &mut seen
            ),
            None
        );
        assert_eq!(
            doorbell(
                &Event::ArtifactDeleted {
                    artifact_id: "a1".into()
                },
                &mut seen
            ),
            None
        );
    }

    #[test]
    fn no_title_publishes_nothing() {
        assert_eq!(
            Publisher::from_env(None, Some("/bin/true".into()), None),
            None
        );
        assert_eq!(
            Publisher::from_env(Some("".into()), Some("/bin/true".into()), None),
            None
        );
        assert_eq!(
            Publisher::from_env(Some("clax".into()), Some("/x/chitin".into()), None),
            Some(Publisher {
                bin: "/x/chitin".into(),
                title: "clax".into()
            })
        );
    }

    #[tokio::test]
    async fn publish_runs_chitin_with_the_doorbells_arguments_alone() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("args");
        let bin = dir.path().join("chitin");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n", out.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let p = Publisher {
            bin,
            title: "clax".into(),
        };
        let d = Doorbell {
            topic: "clax.thread.a1".into(),
            kind: "reply",
            subject: "t-t1".into(),
            dataref: Some("c:00".into()),
        };
        publish(&p, &d, PUBLISH_TIMEOUT).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "publish\n--as\nclax\n--dataref\nc:00\nclax.thread.a1\nreply\nt-t1\n"
        );
    }

    #[tokio::test]
    async fn a_failing_chitin_is_an_error_not_a_panic() {
        let p = Publisher {
            bin: "/bin/false".into(),
            title: "clax".into(),
        };
        let d = Doorbell {
            topic: "t".into(),
            kind: "reply",
            subject: "s".into(),
            dataref: None,
        };
        assert!(publish(&p, &d, PUBLISH_TIMEOUT).await.is_err());
    }
}
