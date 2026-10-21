//! A local daemon's public Clax instances ("remotes") and the artifacts it
//! publishes there.
//!
//! - A **remote** is a hosted Clax this daemon signed in to by device code
//!   (the hosted side is [`crate::device`]): its origin, a short
//!   name (its host unless given), and, once the person approved the
//!   sign-in there, the agent token it holds as that person, with whose it
//!   is. While the sign-in waits, the device code it polls with is kept
//!   instead. The token is an outbound credential, so it is kept as it is
//!   and handed out only as a [`Secret`]; nothing lists it.
//! - A **link** is a local artifact's public twin on one remote: the
//!   remote artifact's ID and address, the last local version pushed
//!   (`pushed_n`) and the public version that push made (`remote_n`),
//!   which a later push compares with the public copy's latest to see
//!   whether someone else published there since; and whether the owner let
//!   agents push new versions without asking again (`auto`).
//! - A **push approval** is the owner's question an agent's push waits on:
//!   which artifact, which remote, which session asked, and the version the
//!   owner was asked about (the push sends no later one). It is used once.
//! - A **remote asset** is where one of a local artifact's assets was
//!   uploaded on a public artifact: the address its pages' references to
//!   it are rewritten to there.
//!
//! None of these is touched until a remote is added. A hosted daemon keeps
//! **publish keys** instead: the idempotency key an agent token's publish
//! named and the version it made ([`Store::publish_key`]), recorded in the
//! transaction that records the version
//! ([`ValidatedPublish::publish_key`](clax_core::publish::ValidatedPublish::publish_key)),
//! so a push whose answer was lost and is sent again is answered that
//! version. The same push sent while the first is still running waits for
//! it ([`Store::lock_publish_key`]).

use super::Store;
use clax_core::content::ArtifactLock;
use clax_core::publish::PublishKey;
use clax_core::secret::Secret;
use clax_core::{CoreError, Result, new_ulid};
use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;

/// The longest remote name, in characters.
pub const MAX_REMOTE_NAME_CHARS: usize = 60;

/// A remote, without its token or device code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Remote {
    pub id: String,
    /// The public origin, normalized (`https://clax.example`).
    pub origin: String,
    pub name: String,
    /// `pending` while the sign-in waits, then `connected`.
    pub status: String,
    /// While pending: the code the person types, the page they approve
    /// on, and when the sign-in expires.
    pub user_code: Option<String>,
    pub verification_url: Option<String>,
    pub sign_in_expires_at: Option<String>,
    /// The seconds between polls the remote asked for.
    pub poll_interval_s: Option<i64>,
    /// Once connected: whose agent token this daemon holds there.
    pub person_public_id: Option<String>,
    pub person_name: Option<String>,
    /// The agent token's ID there (what its People page lists).
    pub grant_id: Option<String>,
    pub created_at: String,
    pub connected_at: Option<String>,
}

const REMOTE_COLS: &str = "id, origin, name, status, user_code, verification_url,
    sign_in_expires_at, poll_interval_s, person_public_id, person_name, grant_id, created_at,
    connected_at";

fn row_remote(r: &Row<'_>) -> rusqlite::Result<Remote> {
    Ok(Remote {
        id: r.get(0)?,
        origin: r.get(1)?,
        name: r.get(2)?,
        status: r.get(3)?,
        user_code: r.get(4)?,
        verification_url: r.get(5)?,
        sign_in_expires_at: r.get(6)?,
        poll_interval_s: r.get(7)?,
        person_public_id: r.get(8)?,
        person_name: r.get(9)?,
        grant_id: r.get(10)?,
        created_at: r.get(11)?,
        connected_at: r.get(12)?,
    })
}

/// A device sign-in a remote started for this daemon.
#[derive(Debug)]
pub struct PendingSignIn {
    pub device_code: Secret,
    pub user_code: String,
    pub verification_url: String,
    pub expires_at: String,
    pub interval_s: i64,
}

/// Whose agent token a finished sign-in handed this daemon.
#[derive(Debug)]
pub struct SignedInAs {
    pub token: Secret,
    pub person_public_id: String,
    pub person_name: Option<String>,
    pub grant_id: String,
}

/// A local artifact's public twin on one remote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RemoteLink {
    pub artifact_id: String,
    pub remote_id: String,
    pub remote_artifact_id: String,
    /// The public artifact's address (`<origin>/a/<id>`).
    pub url: String,
    /// The last local version pushed.
    pub pushed_n: u32,
    /// The public version that push made.
    pub remote_n: u32,
    /// Agents may push new versions without asking the owner again.
    pub auto: bool,
    pub created_at: String,
    pub pushed_at: String,
    /// Local threads stay local; replies and resolves on linked threads
    /// still go up ([`crate::linked`]).
    pub download_only: bool,
    /// The newest activity time read from the public copy's threads.
    pub sync_cursor: Option<String>,
}

const LINK_COLS: &str = "artifact_id, remote_id, remote_artifact_id, url, pushed_n, remote_n,
    auto, created_at, pushed_at, download_only, sync_cursor";

fn row_link(r: &Row<'_>) -> rusqlite::Result<RemoteLink> {
    Ok(RemoteLink {
        artifact_id: r.get(0)?,
        remote_id: r.get(1)?,
        remote_artifact_id: r.get(2)?,
        url: r.get(3)?,
        pushed_n: r.get(4)?,
        remote_n: r.get(5)?,
        auto: r.get(6)?,
        created_at: r.get(7)?,
        pushed_at: r.get(8)?,
        download_only: r.get(9)?,
        sync_cursor: r.get(10)?,
    })
}

/// The owner's question an agent's push waits on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushApproval {
    pub question_id: String,
    pub artifact_id: String,
    pub remote_id: String,
    pub session_id: String,
    /// The artifact's version when the owner was asked: the newest the
    /// approved push sends.
    pub upto: u32,
    pub used_at: Option<String>,
}

/// The longest idempotency key a publish may name, in bytes.
pub const MAX_PUBLISH_KEY: usize = 200;

/// `name` as a remote's name: trimmed, at most [`MAX_REMOTE_NAME_CHARS`]
/// characters, no whitespace inside or control characters.
///
/// # Errors
/// `invalid_args` otherwise.
pub fn check_remote_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty()
        || n.chars().count() > MAX_REMOTE_NAME_CHARS
        || n.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(CoreError::invalid(
            "invalid_args",
            format!("a remote's name is 1 to {MAX_REMOTE_NAME_CHARS} characters, without spaces"),
        ));
    }
    Ok(n.to_string())
}

impl Store {
    /// Records a sign-in to `origin` (normalized) under `name`, waiting for
    /// the person's approval there. A remote of that origin still waiting
    /// is replaced (a new sign-in); a connected one stays as it is.
    ///
    /// # Errors
    /// `remote_exists` for an origin this daemon is signed in to already, or
    /// a name another remote holds; `invalid_args` for a bad name.
    pub fn add_pending_remote(
        &self,
        origin: &str,
        name: &str,
        sign_in: &PendingSignIn,
    ) -> Result<Remote> {
        let name = check_remote_name(name)?;
        self.with_tx(|tx| {
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT id, status FROM remotes WHERE origin = ?1",
                    params![origin],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((id, status)) = &existing {
                if status == "connected" {
                    return Err(CoreError::invalid(
                        "remote_exists",
                        format!("this Clax is signed in to {origin} already: remove it first to sign in again"),
                    ));
                }
                tx.execute("DELETE FROM remotes WHERE id = ?1", params![id])?;
            }
            let taken: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM remotes WHERE name = ?1)",
                params![name],
                |r| r.get(0),
            )?;
            if taken {
                return Err(CoreError::invalid(
                    "remote_exists",
                    format!("a remote is named '{name}' already: give another name"),
                ));
            }
            let id = format!("r_{}", new_ulid().to_lowercase());
            tx.execute(
                "INSERT INTO remotes (id, origin, name, status, device_code, user_code,
                    verification_url, sign_in_expires_at, poll_interval_s, created_at)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    origin,
                    name,
                    sign_in.device_code.expose(),
                    sign_in.user_code,
                    sign_in.verification_url,
                    sign_in.expires_at,
                    sign_in.interval_s,
                    self.now()
                ],
            )?;
            Ok(tx.query_row(
                &format!("SELECT {REMOTE_COLS} FROM remotes WHERE id = ?1"),
                params![id],
                row_remote,
            )?)
        })
    }

    /// The device code a waiting remote's sign-in polls with.
    pub fn remote_device_code(&self, id: &str) -> Result<Option<Secret>> {
        self.with_read(|c| {
            let v: Option<Option<String>> = c
                .query_row(
                    "SELECT device_code FROM remotes WHERE id = ?1 AND status = 'pending'",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(v.flatten().map(Secret::new))
        })
    }

    /// Records that remote `id`'s sign-in finished: the token it holds and
    /// whose it is. The device code and user code are forgotten.
    ///
    /// # Errors
    /// `NotFound` for no remote waiting under `id`.
    pub fn connect_remote(&self, id: &str, signed: &SignedInAs) -> Result<Remote> {
        self.with_tx(|tx| {
            let n = tx.execute(
                "UPDATE remotes SET status = 'connected', token = ?2, person_public_id = ?3,
                    person_name = ?4, grant_id = ?5, connected_at = ?6, device_code = NULL,
                    user_code = NULL, verification_url = NULL, sign_in_expires_at = NULL
                 WHERE id = ?1 AND status = 'pending'",
                params![
                    id,
                    signed.token.expose(),
                    signed.person_public_id,
                    signed.person_name,
                    signed.grant_id,
                    self.now()
                ],
            )?;
            if n == 0 {
                return Err(CoreError::NotFound);
            }
            Ok(tx.query_row(
                &format!("SELECT {REMOTE_COLS} FROM remotes WHERE id = ?1"),
                params![id],
                row_remote,
            )?)
        })
    }

    /// Every remote, oldest first.
    pub fn remotes(&self) -> Result<Vec<Remote>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {REMOTE_COLS} FROM remotes ORDER BY created_at, id"
            ))?;
            Ok(stmt
                .query_map([], row_remote)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The remote named by `key`: its ID, its name or its origin.
    pub fn find_remote(&self, key: &str) -> Result<Option<Remote>> {
        let key = key.trim().trim_end_matches('/');
        self.with_read(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {REMOTE_COLS} FROM remotes WHERE id = ?1 OR name = ?1 OR origin = ?1
                     ORDER BY id = ?1 DESC, name = ?1 DESC LIMIT 1"
                ),
                params![key],
                row_remote,
            )
            .optional()?)
        })
    }

    /// The agent token connected remote `id` holds.
    pub fn remote_token(&self, id: &str) -> Result<Option<Secret>> {
        self.with_read(|c| {
            let v: Option<Option<String>> = c
                .query_row(
                    "SELECT token FROM remotes WHERE id = ?1 AND status = 'connected'",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(v.flatten().map(Secret::new))
        })
    }

    /// Forgets remote `id`, with its links and waiting approvals. Its
    /// linked threads stay as read-only history, and their unsent writes
    /// wait for the owner ([`crate::linked`]). The agent token stays
    /// live there until revoked on its People page.
    ///
    /// # Errors
    /// `NotFound` for no such remote.
    pub fn remove_remote(&self, id: &str) -> Result<Remote> {
        self.with_tx(|tx| {
            let r = tx
                .query_row(
                    &format!("SELECT {REMOTE_COLS} FROM remotes WHERE id = ?1"),
                    params![id],
                    row_remote,
                )
                .optional()?
                .ok_or(CoreError::NotFound)?;
            tx.execute(
                "UPDATE linked_threads SET unlinked_at = ?2 WHERE remote_id = ?1 AND unlinked_at IS NULL",
                params![id, self.now()],
            )?;
            tx.execute("DELETE FROM remote_links WHERE remote_id = ?1", params![id])?;
            tx.execute(
                "DELETE FROM remote_assets WHERE remote_id = ?1",
                params![id],
            )?;
            tx.execute(
                "DELETE FROM push_approvals WHERE remote_id = ?1",
                params![id],
            )?;
            tx.execute("DELETE FROM remotes WHERE id = ?1", params![id])?;
            Ok(r)
        })
    }

    /// Every link, oldest first.
    pub fn all_remote_links(&self) -> Result<Vec<RemoteLink>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {LINK_COLS} FROM remote_links ORDER BY created_at, artifact_id, remote_id"
            ))?;
            Ok(stmt
                .query_map([], row_link)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Artifact `aid`'s links, oldest first.
    pub fn remote_links(&self, aid: &str) -> Result<Vec<RemoteLink>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {LINK_COLS} FROM remote_links WHERE artifact_id = ?1
                 ORDER BY created_at, remote_id"
            ))?;
            Ok(stmt
                .query_map(params![aid], row_link)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Artifact `aid`'s link on remote `rid`.
    pub fn remote_link(&self, aid: &str, rid: &str) -> Result<Option<RemoteLink>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {LINK_COLS} FROM remote_links WHERE artifact_id = ?1 AND remote_id = ?2"
                ),
                params![aid, rid],
                row_link,
            )
            .optional()?)
        })
    }

    /// Records that local version `pushed_n` of `aid` is public version
    /// `remote_n` of `remote_aid` on remote `rid` (at `url`), making the link
    /// or moving it (a push as a new artifact, or onto a fork, moves it to
    /// that artifact). `auto` sets whether agents may push without asking
    /// again; `None` keeps what the link had (false for a new one).
    pub fn record_remote_push(
        &self,
        aid: &str,
        rid: &str,
        remote_aid: &str,
        url: &str,
        (pushed_n, remote_n): (u32, u32),
        auto: Option<bool>,
    ) -> Result<RemoteLink> {
        self.with_tx(|tx| {
            let now = self.now();
            tx.execute(
                "INSERT INTO remote_links (artifact_id, remote_id, remote_artifact_id, url,
                    pushed_n, remote_n, auto, created_at, pushed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, COALESCE(?7, 0), ?8, ?8)
                 ON CONFLICT (artifact_id, remote_id) DO UPDATE SET
                    remote_artifact_id = excluded.remote_artifact_id, url = excluded.url,
                    pushed_n = excluded.pushed_n, remote_n = excluded.remote_n,
                    auto = COALESCE(?7, remote_links.auto), pushed_at = excluded.pushed_at",
                params![aid, rid, remote_aid, url, pushed_n, remote_n, auto, now],
            )?;
            Ok(tx.query_row(
                &format!(
                    "SELECT {LINK_COLS} FROM remote_links WHERE artifact_id = ?1 AND remote_id = ?2"
                ),
                params![aid, rid],
                row_link,
            )?)
        })
    }

    /// Sets whether agents may push `aid` to `rid` without asking again.
    ///
    /// # Errors
    /// `NotFound` for no such link.
    pub fn set_link_auto(&self, aid: &str, rid: &str, auto: bool) -> Result<()> {
        self.with_tx(|tx| {
            let n = tx.execute(
                "UPDATE remote_links SET auto = ?3 WHERE artifact_id = ?1 AND remote_id = ?2",
                params![aid, rid, auto],
            )?;
            if n == 0 {
                return Err(CoreError::NotFound);
            }
            Ok(())
        })
    }

    /// The links whose artifact agents push without asking: what a new
    /// local version of `aid` is pushed to.
    pub fn auto_links(&self, aid: &str) -> Result<Vec<RemoteLink>> {
        Ok(self
            .remote_links(aid)?
            .into_iter()
            .filter(|l| l.auto)
            .collect())
    }

    /// Records that the owner's question `qid` decides whether session
    /// `sid` may push `aid` to `rid`, up to version `upto` (what the
    /// question names).
    pub fn create_push_approval(
        &self,
        qid: &str,
        aid: &str,
        rid: &str,
        sid: &str,
        upto: u32,
    ) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO push_approvals (question_id, artifact_id, remote_id, session_id,
                    upto, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![qid, aid, rid, sid, upto, self.now()],
            )?;
            Ok(())
        })
    }

    /// The push approval question `qid` decides.
    pub fn push_approval(&self, qid: &str) -> Result<Option<PushApproval>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT question_id, artifact_id, remote_id, session_id, upto, used_at
                 FROM push_approvals WHERE question_id = ?1",
                params![qid],
                |r| {
                    Ok(PushApproval {
                        question_id: r.get(0)?,
                        artifact_id: r.get(1)?,
                        remote_id: r.get(2)?,
                        session_id: r.get(3)?,
                        upto: r.get(4)?,
                        used_at: r.get(5)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// Spends push approval `qid`: `true` the first time, `false` once spent
    /// (or for none).
    pub fn use_push_approval(&self, qid: &str) -> Result<bool> {
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE push_approvals SET used_at = ?2 WHERE question_id = ?1 AND used_at IS NULL",
                params![qid, self.now()],
            )? == 1)
        })
    }

    /// The open push approval of session `sid` for `aid` on `rid`, if one
    /// waits: asking again waits on the same question.
    pub fn open_push_approval(&self, sid: &str, aid: &str, rid: &str) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT p.question_id FROM push_approvals p JOIN questions q ON q.id = p.question_id
                 WHERE p.session_id = ?1 AND p.artifact_id = ?2 AND p.remote_id = ?3
                   AND p.used_at IS NULL AND q.status IN ('open', 'answered')
                 ORDER BY p.created_at DESC LIMIT 1",
                params![sid, aid, rid],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// Where local asset `asset_id` was uploaded on public artifact
    /// `remote_aid` of remote `rid`, if it was.
    pub fn remote_asset(
        &self,
        rid: &str,
        remote_aid: &str,
        asset_id: &str,
    ) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT url FROM remote_assets
                 WHERE remote_id = ?1 AND remote_artifact_id = ?2 AND asset_id = ?3",
                params![rid, remote_aid, asset_id],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// Records that local asset `asset_id` was uploaded on public artifact
    /// `remote_aid` of remote `rid`, at `url`.
    pub fn record_remote_asset(
        &self,
        rid: &str,
        remote_aid: &str,
        asset_id: &str,
        url: &str,
    ) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO remote_assets (remote_id, remote_artifact_id, asset_id, url, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (remote_id, remote_artifact_id, asset_id) DO UPDATE SET url = excluded.url",
                params![rid, remote_aid, asset_id, url, self.now()],
            )?;
            Ok(())
        })
    }

    /// On a hosted daemon: the artifact and version that agent token
    /// `grant_id`'s publish keyed `key` made, if one did.
    pub fn publish_key(&self, grant_id: &str, key: &str) -> Result<Option<(String, u32)>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT artifact_id, n FROM publish_keys WHERE grant_id = ?1 AND key = ?2",
                params![grant_id, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }

    /// On a hosted daemon: waits for, then holds, the lock of agent token
    /// `grant_id`'s idempotency key `key`. A keyed publish holds it from
    /// its look-up of the key ([`Store::publish_key`]) to the end of the
    /// transaction that records its version with the key, so the same
    /// publish sent again meanwhile waits, then answers the version the
    /// first made.
    pub async fn lock_publish_key(&self, grant_id: &str, key: &str) -> ArtifactLock<'_> {
        let _lane = super::exec::WriteLane::waiting();
        self.publish_key_locks
            .lock_key(&format!("{grant_id}\n{key}"))
            .await
    }
}

/// Records, in `tx` (the transaction recording the version), that the
/// publish keyed `key` made version `n` of `aid`. A key whose recorded
/// artifact is gone (deleted, or never recorded) is taken over: the
/// publish it names is made again, as [`Store::publish_key`]'s callers
/// replay only a version that still exists.
///
/// # Errors
/// `publish_key_used` when the key is recorded for a live artifact
/// already (another process's publish with the key made a version first).
pub(super) fn record_publish_key_in(
    tx: &rusqlite::Connection,
    key: &PublishKey,
    aid: &str,
    n: u32,
    now: &str,
) -> Result<()> {
    let made = tx.execute(
        "INSERT INTO publish_keys (grant_id, key, artifact_id, n, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (grant_id, key) DO UPDATE SET artifact_id = excluded.artifact_id,
             n = excluded.n, created_at = excluded.created_at
         WHERE NOT EXISTS (SELECT 1 FROM artifacts
                           WHERE id = publish_keys.artifact_id AND deleted_at IS NULL)",
        params![key.grant_id, key.key, aid, n, now],
    )?;
    if made == 0 {
        return Err(CoreError::invalid(
            "publish_key_used",
            "this Idempotency-Key named another publish, which made a version first",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::Home;
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let st = Store::open_with_clock(&Home::at(dir.path().join("ax")), clock).unwrap();
        (dir, st)
    }

    fn sign_in(code: &str) -> PendingSignIn {
        PendingSignIn {
            device_code: Secret::new(code.into()),
            user_code: "WXYZ-4821".into(),
            verification_url: "https://clax.example/device?code=WXYZ-4821".into(),
            expires_at: "2026-10-12T10:10:00.000Z".into(),
            interval_s: 5,
        }
    }

    fn code_of<T: std::fmt::Debug>(r: Result<T>) -> String {
        match r.unwrap_err() {
            CoreError::Invalid { code, .. } => code.to_string(),
            CoreError::NotFound => "not_found".into(),
            e => format!("{e:?}"),
        }
    }

    #[test]
    fn a_remote_waits_then_connects_and_keeps_its_token_out_of_views() {
        let (_d, st) = store();
        let r = st
            .add_pending_remote("https://clax.example", "clax.example", &sign_in("cxd_1"))
            .unwrap();
        assert_eq!(r.status, "pending");
        assert_eq!(
            st.remote_device_code(&r.id).unwrap().unwrap().expose(),
            "cxd_1"
        );
        assert!(st.remote_token(&r.id).unwrap().is_none());
        // A new sign-in replaces one still waiting.
        let r = st
            .add_pending_remote("https://clax.example", "clax.example", &sign_in("cxd_2"))
            .unwrap();
        assert_eq!(st.remotes().unwrap().len(), 1);
        let c = st
            .connect_remote(
                &r.id,
                &SignedInAs {
                    token: Secret::new("cxa_secret".into()),
                    person_public_id: "p1".into(),
                    person_name: Some("Ada".into()),
                    grant_id: "g_1".into(),
                },
            )
            .unwrap();
        assert_eq!(c.status, "connected");
        assert_eq!(c.user_code, None);
        assert!(st.remote_device_code(&r.id).unwrap().is_none());
        assert_eq!(
            st.remote_token(&r.id).unwrap().unwrap().expose(),
            "cxa_secret"
        );
        let shown = serde_json::to_string(&st.remotes().unwrap()).unwrap();
        assert!(
            !shown.contains("cxa_secret") && !shown.contains("cxd_"),
            "{shown}"
        );
        assert_eq!(st.find_remote("clax.example").unwrap().unwrap().id, r.id);
        assert_eq!(
            st.find_remote("https://clax.example/").unwrap().unwrap().id,
            r.id
        );
        assert_eq!(
            code_of(st.add_pending_remote("https://clax.example", "other", &sign_in("cxd_3"))),
            "remote_exists"
        );
        assert_eq!(
            code_of(st.add_pending_remote("https://b.example", "clax.example", &sign_in("cxd_4"))),
            "remote_exists"
        );
        assert_eq!(
            code_of(st.add_pending_remote("https://b.example", "a b", &sign_in("cxd_4"))),
            "invalid_args"
        );
    }

    #[test]
    fn links_record_pushes_and_go_with_their_remote() {
        let (_d, st) = store();
        let r = st
            .add_pending_remote("https://clax.example", "pub", &sign_in("cxd_1"))
            .unwrap();
        let l = st
            .record_remote_push("a1", &r.id, "b1", "https://clax.example/a/b1", (1, 1), None)
            .unwrap();
        assert!(!l.auto);
        let l = st
            .record_remote_push(
                "a1",
                &r.id,
                "b1",
                "https://clax.example/a/b1",
                (3, 4),
                Some(true),
            )
            .unwrap();
        assert_eq!((l.pushed_n, l.remote_n, l.auto), (3, 4, true));
        let l = st
            .record_remote_push("a1", &r.id, "b1", "https://clax.example/a/b1", (4, 5), None)
            .unwrap();
        assert!(l.auto, "kept");
        assert_eq!(st.auto_links("a1").unwrap().len(), 1);
        st.set_link_auto("a1", &r.id, false).unwrap();
        assert!(st.auto_links("a1").unwrap().is_empty());
        assert_eq!(code_of(st.set_link_auto("a2", &r.id, true)), "not_found");
        st.create_push_approval("q1", "a1", &r.id, "s1", 3).unwrap();
        assert_eq!(st.push_approval("q1").unwrap().unwrap().upto, 3);
        st.record_remote_asset(&r.id, "b1", "as1", "https://c.example/_blob/x?k=1")
            .unwrap();
        assert_eq!(
            st.remote_asset(&r.id, "b1", "as1").unwrap().as_deref(),
            Some("https://c.example/_blob/x?k=1")
        );
        assert!(st.remote_asset(&r.id, "b2", "as1").unwrap().is_none());
        assert!(st.use_push_approval("q1").unwrap());
        assert!(!st.use_push_approval("q1").unwrap());
        assert!(st.push_approval("q1").unwrap().unwrap().used_at.is_some());
        st.remove_remote(&r.id).unwrap();
        assert!(st.remote_links("a1").unwrap().is_empty());
        assert!(st.push_approval("q1").unwrap().is_none());
        assert!(st.remote_asset(&r.id, "b1", "as1").unwrap().is_none());
        assert_eq!(code_of(st.remove_remote(&r.id)), "not_found");
    }

    /// A publish naming `key` of grant `g1`, at `if_version`.
    fn keyed(key: &str, if_version: Option<u32>) -> clax_core::publish::ValidatedPublish {
        let req: clax_core::publish::PublishRequest = serde_json::from_value(serde_json::json!({
            "title": "Notes",
            "if_version": if_version,
            "files": {"index.html": {"content": key, "encoding": "utf8"}}
        }))
        .unwrap();
        let mut p = clax_core::publish::validate(req).unwrap();
        p.publish_key = Some(PublishKey {
            grant_id: "g1".into(),
            key: key.into(),
        });
        p
    }

    #[test]
    fn a_publish_key_is_recorded_with_the_version_it_made() {
        let (_d, st) = store();
        let ctx = &clax_core::audit::AuditCtx::DAEMON;
        assert!(st.publish_key("g1", "k1").unwrap().is_none());
        let (a, _) = st.create_artifact(ctx, keyed("k1", None), None).unwrap();
        assert_eq!(st.publish_key("g1", "k1").unwrap(), Some((a.id.clone(), 1)));
        let id = clax_core::ArtifactId::parse(&a.id).unwrap();
        st.publish_version(ctx, &id, keyed("k2", Some(1)), None)
            .unwrap();
        assert_eq!(st.publish_key("g1", "k2").unwrap(), Some((a.id.clone(), 2)));
        assert!(st.publish_key("g2", "k1").unwrap().is_none());
        // A key that made a version makes no other.
        assert_eq!(
            code_of(st.publish_version(ctx, &id, keyed("k1", Some(2)), None)),
            "publish_key_used"
        );
        assert_eq!(st.get_artifact(&id).unwrap().unwrap().current_version, 2);
        assert_eq!(st.publish_key("g1", "k1").unwrap(), Some((a.id, 1)));
    }

    #[test]
    fn a_publish_key_whose_artifact_was_deleted_makes_a_new_one() {
        let (_d, st) = store();
        let ctx = &clax_core::audit::AuditCtx::DAEMON;
        let (a, _) = st.create_artifact(ctx, keyed("k1", None), None).unwrap();
        let id = clax_core::ArtifactId::parse(&a.id).unwrap();
        st.delete_artifact(ctx, &id).unwrap();
        let (b, _) = st.create_artifact(ctx, keyed("k1", None), None).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(st.publish_key("g1", "k1").unwrap(), Some((b.id, 1)));
        assert!(st.zero_version_artifacts().unwrap().is_empty());
    }

    #[test]
    fn a_create_whose_key_is_taken_leaves_no_artifact() {
        let (_d, st) = store();
        let ctx = &clax_core::audit::AuditCtx::DAEMON;
        let (a, _) = st.create_artifact(ctx, keyed("k1", None), None).unwrap();
        assert_eq!(
            code_of(st.create_artifact(ctx, keyed("k1", None), None)),
            "publish_key_used"
        );
        assert!(st.zero_version_artifacts().unwrap().is_empty());
        assert_eq!(st.publish_key("g1", "k1").unwrap(), Some((a.id, 1)));
    }

    #[test]
    fn a_publish_key_is_held_by_one_publish_at_a_time() {
        let (_d, st) = store();
        let held = clax_core::content::ready(st.lock_publish_key("g1", "k")).unwrap();
        assert!(clax_core::content::ready(st.lock_publish_key("g1", "k")).is_none());
        assert!(clax_core::content::ready(st.lock_publish_key("g2", "k")).is_some());
        drop(held);
        assert!(clax_core::content::ready(st.lock_publish_key("g1", "k")).is_some());
    }
}
