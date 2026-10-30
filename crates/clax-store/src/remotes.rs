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
    /// `pending` while the sign-in waits, then `connected`; `signed_out`
    /// once the remote refused its token (revoked there,
    /// [`Store::sign_out_remote`]), until a new sign-in.
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
    /// The kind its sign-in named ([`CLIENT_KIND_APP`] or
    /// [`CLIENT_KIND_AGENT`]); `None` for a remote signed in before kinds
    /// were kept, until the sync asks the remote. Not served.
    #[serde(skip)]
    pub client_kind: Option<String>,
}

/// [`Remote::client_kind`] of the Clax app's sign-in: the remote's token
/// is its person's device, which reads threads unfiltered, as the person's
/// browser does.
pub const CLIENT_KIND_APP: &str = "clax-app";

/// [`Remote::client_kind`] of a sign-in that named no kind: the remote's
/// token is an agent token, whose thread reads the remote filters by the
/// person's approvals there.
pub const CLIENT_KIND_AGENT: &str = "agent";

const REMOTE_COLS: &str = "id, origin, name, status, user_code, verification_url,
    sign_in_expires_at, poll_interval_s, person_public_id, person_name, grant_id, created_at,
    connected_at, client_kind";

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
        client_kind: r.get(13)?,
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
    /// A followed link ([`Store::follow_remote`]): `artifact_id` is the
    /// local mirror of a remote artifact's threads, not a pushed artifact.
    pub followed: bool,
    /// A followed link no local session watches, since when; `None`
    /// while one does (and for a pushed link).
    pub unwatched_at: Option<String>,
}

impl RemoteLink {
    /// Whether the sync keeps this link: a pushed one, or a followed one
    /// some local session watches.
    pub fn synced(&self) -> bool {
        !self.followed || self.unwatched_at.is_none()
    }
}

/// How long a followed link no local session watches keeps its mirror
/// ([`Store::mirrors_to_drop`]).
pub const MIRROR_KEPT: chrono::Duration = chrono::Duration::days(7);

/// The remote artifact a mirror holds the threads of ([`Store::mirror_of`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorOf {
    pub remote_artifact_id: String,
    /// Its address there.
    pub url: String,
    pub origin: String,
    pub name: String,
}

/// A live local session watching the mirror of a followed link: whom the
/// sync opens a session there for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowWatcher {
    /// The mirror.
    pub artifact_id: String,
    pub remote_artifact_id: String,
    pub session_id: String,
    /// The session's name, else its harness.
    pub name: String,
    pub harness: String,
}

const LINK_COLS: &str = "artifact_id, remote_id, remote_artifact_id, url, pushed_n, remote_n,
    auto, created_at, pushed_at, download_only, sync_cursor, followed, unwatched_at";

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
        followed: r.get(11)?,
        unwatched_at: r.get(12)?,
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
    /// is replaced (a new sign-in); a signed-out one waits again under its
    /// own ID, keeping its links; a connected one stays as it is.
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
            let mut keep: Option<String> = None;
            if let Some((id, status)) = &existing {
                match status.as_str() {
                    "connected" => {
                        return Err(CoreError::invalid(
                            "remote_exists",
                            format!("this Clax is signed in to {origin} already: remove it first to sign in again"),
                        ));
                    }
                    "signed_out" => keep = Some(id.clone()),
                    _ => {
                        tx.execute("DELETE FROM remotes WHERE id = ?1", params![id])?;
                    }
                }
            }
            let taken: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM remotes WHERE name = ?1 AND id IS NOT ?2)",
                params![name, keep],
                |r| r.get(0),
            )?;
            if taken {
                return Err(CoreError::invalid(
                    "remote_exists",
                    format!("a remote is named '{name}' already: give another name"),
                ));
            }
            if let Some(id) = keep {
                tx.execute(
                    "UPDATE remotes SET name = ?2, status = 'pending', token = NULL,
                        device_code = ?3, user_code = ?4, verification_url = ?5,
                        sign_in_expires_at = ?6, poll_interval_s = ?7
                     WHERE id = ?1",
                    params![
                        id,
                        name,
                        sign_in.device_code.expose(),
                        sign_in.user_code,
                        sign_in.verification_url,
                        sign_in.expires_at,
                        sign_in.interval_s
                    ],
                )?;
                return Ok(tx.query_row(
                    &format!("SELECT {REMOTE_COLS} FROM remotes WHERE id = ?1"),
                    params![id],
                    row_remote,
                )?);
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

    /// Records `kind` ([`CLIENT_KIND_APP`] or [`CLIENT_KIND_AGENT`]) as the
    /// kind remote `id`'s sign-in named.
    pub fn set_remote_client_kind(&self, id: &str, kind: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE remotes SET client_kind = ?2 WHERE id = ?1",
                params![id, kind],
            )?;
            Ok(())
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

    /// Marks connected remote `id` signed out: the remote refused its
    /// token (revoked there), which is forgotten. Its links stay, for the
    /// sign-in that connects it again ([`Store::add_pending_remote`]).
    /// Whether it was connected.
    pub fn sign_out_remote(&self, id: &str) -> Result<bool> {
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE remotes SET status = 'signed_out', token = NULL
                 WHERE id = ?1 AND status = 'connected'",
                params![id],
            )? > 0)
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
            // Its followed artifacts' mirrors go with it.
            tx.execute(
                "UPDATE artifacts SET deleted_at = ?2
                 WHERE remote_mirror = 1 AND deleted_at IS NULL AND id IN
                    (SELECT artifact_id FROM remote_links WHERE remote_id = ?1 AND followed = 1)",
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

    /// Follows artifact `remote_aid` of remote `rid` (at `url`, titled
    /// `title` there): its local mirror's ID. A mirror holds the remote
    /// artifact's threads and comments and nothing else (no version, no
    /// page, no file; `artifacts.remote_mirror`), and is in no listing.
    /// The mirror and its followed link are made the first time; later
    /// they are reused, the link watched again (`unwatched_at` cleared)
    /// and its address and the mirror's title brought up to date.
    ///
    /// # Errors
    /// `pushed_here` when an artifact of this home is pushed as
    /// `remote_aid` there: its local artifact is the one to watch.
    pub fn follow_remote(
        &self,
        rid: &str,
        remote_aid: &str,
        url: &str,
        title: &str,
    ) -> Result<String> {
        self.with_tx(|tx| self.follow_in(tx, rid, remote_aid, url, title))
    }

    /// [`Store::follow_remote`], then session `sid` watches the mirror
    /// (`replies_armed`, recorded under `ctx` as [`Store::watch`] records
    /// it), in one transaction: no reader sees the link followed again
    /// with its watch not yet made. The mirror and the watch.
    ///
    /// # Errors
    /// As [`Store::follow_remote`] and [`Store::watch`]; nothing is
    /// written then.
    #[allow(clippy::too_many_arguments)]
    pub fn follow_and_watch(
        &self,
        ctx: &clax_core::audit::AuditCtx,
        sid: &str,
        (rid, remote_aid): (&str, &str),
        url: &str,
        title: &str,
        replies_armed: bool,
    ) -> Result<(String, clax_core::model::Watch)> {
        self.with_tx(|tx| {
            let m = self.follow_in(tx, rid, remote_aid, url, title)?;
            let id = clax_core::ArtifactId::parse(&m)?;
            let w = super::watches::watch_in(self, tx, ctx, sid, &id, replies_armed)?;
            Ok((m, w))
        })
    }

    /// [`Store::follow_remote`] in `tx`.
    fn follow_in(
        &self,
        tx: &rusqlite::Transaction<'_>,
        rid: &str,
        remote_aid: &str,
        url: &str,
        title: &str,
    ) -> Result<String> {
        let title = if title.trim().is_empty() {
            remote_aid
        } else {
            title
        };
        let now = self.now();
        let pushed: Option<String> = tx
            .query_row(
                "SELECT artifact_id FROM remote_links
                 WHERE remote_id = ?1 AND remote_artifact_id = ?2 AND followed = 0",
                params![rid, remote_aid],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(aid) = pushed {
            return Err(CoreError::invalid(
                "pushed_here",
                format!("{url} is this Clax's artifact {aid}, pushed there: watch {aid}"),
            ));
        }
        let mirror: Option<String> = tx
            .query_row(
                "SELECT l.artifact_id FROM remote_links l JOIN artifacts a ON a.id = l.artifact_id
                 WHERE l.remote_id = ?1 AND l.remote_artifact_id = ?2 AND l.followed = 1
                   AND a.deleted_at IS NULL",
                params![rid, remote_aid],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(m) = mirror {
            tx.execute(
                "UPDATE remote_links SET unwatched_at = NULL, url = ?3
                 WHERE artifact_id = ?1 AND remote_id = ?2",
                params![m, rid, url],
            )?;
            tx.execute(
                "UPDATE artifacts SET title = ?2 WHERE id = ?1",
                params![m, title],
            )?;
            return Ok(m);
        }
        let id = clax_core::ArtifactId::generate();
        tx.execute(
            "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, pinned,
                capabilities_json, contract_version, owner_session_id, remote_mirror)
             VALUES (?1, ?2, ?3, ?3, 0, 0, '{}', ?4, NULL, 1)",
            params![id.as_str(), title, now, clax_core::model::CONTRACT_VERSION],
        )?;
        tx.execute(
            "INSERT INTO remote_links (artifact_id, remote_id, remote_artifact_id, url,
                pushed_n, remote_n, auto, created_at, pushed_at, followed)
             VALUES (?1, ?2, ?3, ?4, 0, 0, 0, ?5, ?5, 1)",
            params![id.as_str(), rid, remote_aid, url, now],
        )?;
        Ok(id.as_str().to_string())
    }

    /// The mirror of artifact `remote_aid` of remote `rid`, while this
    /// home follows it (watched or not).
    pub fn remote_mirror(&self, rid: &str, remote_aid: &str) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT l.artifact_id FROM remote_links l JOIN artifacts a ON a.id = l.artifact_id
                 WHERE l.remote_id = ?1 AND l.remote_artifact_id = ?2 AND l.followed = 1
                   AND a.deleted_at IS NULL",
                params![rid, remote_aid],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// Stops following the remote artifact mirror `mirror_id` holds, while
    /// no live local session watches it: from now (`unwatched_at`) the sync
    /// leaves it alone, and [`Store::mirrors_to_drop`] names it once
    /// [`MIRROR_KEPT`] has passed. A live watch (one made since the caller
    /// looked) keeps it followed. Whether it stopped following it.
    pub fn unfollow_remote(&self, mirror_id: &str) -> Result<bool> {
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE remote_links SET unwatched_at = ?2
                 WHERE artifact_id = ?1 AND followed = 1 AND unwatched_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM watches w JOIN sessions s ON s.id = w.session_id
                                   WHERE w.artifact_id = ?1 AND s.ended_at IS NULL)",
                params![mirror_id, self.now()],
            )? > 0)
        })
    }

    /// The mirrors no local session has watched for [`MIRROR_KEPT`] by
    /// `now`, oldest first.
    pub fn mirrors_to_drop(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<String>> {
        let before = clax_core::time::stamp(now - MIRROR_KEPT);
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT l.artifact_id FROM remote_links l
                 WHERE l.followed = 1 AND l.unwatched_at IS NOT NULL AND l.unwatched_at <= ?1
                   AND NOT EXISTS (SELECT 1 FROM watches w JOIN sessions s ON s.id = w.session_id
                                   WHERE w.artifact_id = l.artifact_id AND s.ended_at IS NULL)
                 ORDER BY l.unwatched_at, l.artifact_id",
            )?;
            Ok(stmt
                .query_map(params![before], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Drops mirror `mirror_id` while [`Store::mirrors_to_drop`] would
    /// still name it by `now` (no live watch, unwatched for
    /// [`MIRROR_KEPT`]): its followed link goes, its threads end as
    /// history, and the mirror is deleted. Whether it dropped it.
    pub fn drop_mirror(&self, mirror_id: &str, now: chrono::DateTime<chrono::Utc>) -> Result<bool> {
        let before = clax_core::time::stamp(now - MIRROR_KEPT);
        self.with_tx(|tx| {
            let now = self.now();
            let due: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM remote_links l
                    WHERE l.artifact_id = ?1 AND l.followed = 1
                      AND l.unwatched_at IS NOT NULL AND l.unwatched_at <= ?2
                      AND NOT EXISTS (SELECT 1 FROM watches w JOIN sessions s ON s.id = w.session_id
                                      WHERE w.artifact_id = l.artifact_id AND s.ended_at IS NULL))",
                params![mirror_id, before],
                |r| r.get(0),
            )?;
            if !due {
                return Ok(false);
            }
            let n = tx.execute(
                "UPDATE artifacts SET deleted_at = ?2
                 WHERE id = ?1 AND remote_mirror = 1 AND deleted_at IS NULL",
                params![mirror_id, now],
            )?;
            tx.execute(
                "UPDATE linked_threads SET unlinked_at = ?2 WHERE artifact_id = ?1 AND unlinked_at IS NULL",
                params![mirror_id, now],
            )?;
            tx.execute(
                "DELETE FROM remote_links WHERE artifact_id = ?1 AND followed = 1",
                params![mirror_id],
            )?;
            Ok(n > 0)
        })
    }

    /// The live local sessions watching the mirror of each watched
    /// followed link of remote `rid`, by link then session.
    pub fn follow_watchers(&self, rid: &str) -> Result<Vec<FollowWatcher>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT l.artifact_id, l.remote_artifact_id, s.id, COALESCE(s.name, s.harness), s.harness
                 FROM remote_links l
                 JOIN watches w ON w.artifact_id = l.artifact_id
                 JOIN sessions s ON s.id = w.session_id
                 WHERE l.remote_id = ?1 AND l.followed = 1 AND l.unwatched_at IS NULL
                   AND s.ended_at IS NULL
                 ORDER BY l.created_at, l.artifact_id, s.id",
            )?;
            Ok(stmt
                .query_map(params![rid], |r| {
                    Ok(FollowWatcher {
                        artifact_id: r.get(0)?,
                        remote_artifact_id: r.get(1)?,
                        session_id: r.get(2)?,
                        name: r.get(3)?,
                        harness: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// What mirror `aid` mirrors, when it is a followed remote artifact's
    /// mirror: the artifact there (its ID and address) and its remote's
    /// origin and name.
    pub fn mirror_of(&self, aid: &str) -> Result<Option<MirrorOf>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT l.remote_artifact_id, l.url, r.origin, r.name
                 FROM remote_links l JOIN remotes r ON r.id = l.remote_id
                 WHERE l.artifact_id = ?1 AND l.followed = 1",
                params![aid],
                |r| {
                    Ok(MirrorOf {
                        remote_artifact_id: r.get(0)?,
                        url: r.get(1)?,
                        origin: r.get(2)?,
                        name: r.get(3)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// Whether `aid` is a followed remote artifact's mirror.
    pub fn is_remote_mirror(&self, aid: &str) -> Result<bool> {
        self.with_read(|c| is_mirror_in(c, aid))
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

    /// Links `aid` to public artifact `remote_aid` on remote `rid` (at
    /// `url`) without a push: local version `local_n` is public version
    /// `remote_n`, the base the next push builds on (an app got from the
    /// remote, or one whose public copy this device took). Makes the link
    /// or moves it; agents may not push it without asking (`auto` off).
    /// `pushed_at` becomes now, when the base was set. Moved to another
    /// public artifact, the link starts over as a new one would: the old
    /// copy's threads end as read-only history (as
    /// [`Store::unlink_remote`] ends them), its read starts from nothing,
    /// it is two-way, and only threads started from now on go up.
    pub fn link_remote(
        &self,
        aid: &str,
        rid: &str,
        remote_aid: &str,
        url: &str,
        local_n: u32,
        remote_n: u32,
    ) -> Result<RemoteLink> {
        self.with_tx(|tx| {
            let now = self.now();
            let moved = tx.execute(
                "UPDATE remote_links SET sync_cursor = NULL, download_only = 0, threads_since = ?4
                 WHERE artifact_id = ?1 AND remote_id = ?2 AND remote_artifact_id <> ?3",
                params![aid, rid, remote_aid, now],
            )?;
            if moved > 0 {
                tx.execute(
                    "UPDATE linked_threads SET unlinked_at = ?3
                     WHERE artifact_id = ?1 AND remote_id = ?2 AND unlinked_at IS NULL",
                    params![aid, rid, now],
                )?;
            }
            tx.execute(
                "INSERT INTO remote_links (artifact_id, remote_id, remote_artifact_id, url,
                    pushed_n, remote_n, auto, created_at, pushed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?7)
                 ON CONFLICT (artifact_id, remote_id) DO UPDATE SET
                    remote_artifact_id = excluded.remote_artifact_id, url = excluded.url,
                    pushed_n = excluded.pushed_n, remote_n = excluded.remote_n, auto = 0,
                    pushed_at = excluded.pushed_at",
                params![aid, rid, remote_aid, url, local_n, remote_n, now],
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

/// Whether `aid` is a followed remote artifact's mirror, on `c`.
pub(crate) fn is_mirror_in(c: &rusqlite::Connection, aid: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT remote_mirror FROM artifacts WHERE id = ?1",
        params![aid],
        |r| r.get::<_, bool>(0),
    )
    .optional()?
    .unwrap_or(false))
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

    /// A revoked token signs a remote out: its token is forgotten and its
    /// links stay; the next sign-in waits under the same ID, so the links
    /// are the reconnected remote's.
    #[test]
    fn a_signed_out_remote_keeps_its_links_for_the_next_sign_in() {
        let (_d, st) = store();
        let signed = |t: &str| SignedInAs {
            token: Secret::new(t.into()),
            person_public_id: "p1".into(),
            person_name: Some("Ada".into()),
            grant_id: "g_1".into(),
        };
        let r = st
            .add_pending_remote("https://clax.example", "pub", &sign_in("cxd_1"))
            .unwrap();
        st.connect_remote(&r.id, &signed("cxa_1")).unwrap();
        st.link_remote("a1", &r.id, "b1", "https://clax.example/a/b1", 1, 1)
            .unwrap();
        assert!(st.sign_out_remote(&r.id).unwrap());
        assert!(!st.sign_out_remote(&r.id).unwrap(), "only once");
        let out = st.find_remote(&r.id).unwrap().unwrap();
        assert_eq!(out.status, "signed_out");
        assert!(st.remote_token(&r.id).unwrap().is_none());
        assert_eq!(st.remote_links("a1").unwrap().len(), 1);
        let again = st
            .add_pending_remote("https://clax.example", "pub", &sign_in("cxd_2"))
            .unwrap();
        assert_eq!(
            (again.id.as_str(), again.status.as_str()),
            (r.id.as_str(), "pending")
        );
        st.connect_remote(&r.id, &signed("cxa_2")).unwrap();
        assert_eq!(st.remote_token(&r.id).unwrap().unwrap().expose(), "cxa_2");
        assert_eq!(st.remote_links("a1").unwrap()[0].remote_id, r.id);
        // A pending remote is not signed out.
        let p = st
            .add_pending_remote("https://b.example", "b", &sign_in("cxd_3"))
            .unwrap();
        assert!(!st.sign_out_remote(&p.id).unwrap());
    }

    #[test]
    fn link_remote_without_push_sets_base() {
        let (_d, st) = store();
        let r = st
            .add_pending_remote("https://clax.example", "pub", &sign_in("cxd_1"))
            .unwrap();
        let l = st
            .link_remote("a1", &r.id, "b1", "https://clax.example/a/b1", 1, 6)
            .unwrap();
        assert_eq!(
            (
                l.remote_artifact_id.as_str(),
                l.pushed_n,
                l.remote_n,
                l.auto
            ),
            ("b1", 1, 6, false)
        );
        assert_eq!(st.remote_link("a1", &r.id).unwrap(), Some(l.clone()));
        // Linked again (the public copy taken anew), on an auto link: the
        // base moves and agents must ask again.
        st.set_link_auto("a1", &r.id, true).unwrap();
        let again = st
            .link_remote("a1", &r.id, "b2", "https://clax.example/a/b2", 3, 9)
            .unwrap();
        assert_eq!(
            (
                again.remote_artifact_id.as_str(),
                again.url.as_str(),
                again.pushed_n,
                again.remote_n,
                again.auto
            ),
            ("b2", "https://clax.example/a/b2", 3, 9, false)
        );
        assert_eq!(again.created_at, l.created_at);
        assert_eq!(st.remote_links("a1").unwrap(), vec![again]);
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

    /// A followed remote artifact's mirror, which never has a version, is
    /// no crashed create: the start-up sweep and `clax doctor` leave it,
    /// with or without threads, and no listing names it.
    #[test]
    fn a_mirror_survives_the_zero_version_sweeps_and_is_in_no_listing() {
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let (_d, st) = crate::test_util::store_with_clock(clock.clone());
        let r = st
            .add_pending_remote("https://clax.example", "pub", &sign_in("cxd_1"))
            .unwrap();
        let rid = r.id.as_str();
        let bare = st
            .follow_remote(rid, "b1", "https://clax.example/a/b1", "Bare")
            .unwrap();
        let threaded = st
            .follow_remote(rid, "b2", "https://clax.example/a/b2", "Threaded")
            .unwrap();
        let link = st.remote_link(&threaded, rid).unwrap().unwrap();
        st.mirror_thread(
            &clax_core::audit::AuditCtx::DAEMON,
            &link,
            ("pub", "https://clax.example"),
            1,
            false,
            crate::linked::MirrorThread {
                remote_thread_id: "01J9Z3K4M5N6P7Q8R9S0T1V2W3".into(),
                remote_version: 1,
                anchor: serde_json::from_value(serde_json::json!({"kind": "artifact"})).unwrap(),
                first: crate::linked::MirrorComment {
                    remote_comment_id: "c1".into(),
                    author: crate::linked::RemoteAuthor {
                        kind: "viewer".into(),
                        public_id: None,
                        name: "Ada".into(),
                    },
                    body: "there".into(),
                },
            },
        )
        .unwrap();
        // A crashed create beside them is still found and swept.
        st.with_tx(|tx| {
            tx.execute(
                "INSERT INTO artifacts (id, title, created_at, updated_at, current_version, pinned,
                    capabilities_json, contract_version)
                 VALUES ('7q3k9mzx2b4t', 'Crashed', ?1, ?1, 0, 0, '{}', '1')",
                params![st.now()],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(st.zero_version_artifacts().unwrap(), ["7q3k9mzx2b4t"]);
        clock.advance(3600);
        let swept = st
            .block_on(st.sweep_zero_version_artifacts(chrono::Duration::seconds(60)))
            .unwrap();
        assert_eq!(swept, 1);
        assert_eq!(st.delete_zero_version_artifacts().unwrap(), 0);
        for (m, raid) in [(&bare, "b1"), (&threaded, "b2")] {
            assert!(st.is_remote_mirror(m).unwrap(), "{m}");
            assert_eq!(st.remote_mirror(rid, raid).unwrap().as_ref(), Some(m));
        }
        assert!(st.list_artifacts().unwrap().is_empty());
    }

    /// Unfollowing keeps a link followed while a live session watches its
    /// mirror (a watch made after the caller looked), and a mirror is
    /// dropped only while it is still unwatched past the week.
    #[test]
    fn unfollowing_and_dropping_a_mirror_wait_for_no_live_watch() {
        let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
        let (_d, st) = crate::test_util::store_with_clock(clock.clone());
        let sid = crate::test_util::session(&st, "claude", "h1");
        let follow = || {
            st.follow_and_watch(
                &clax_core::audit::AuditCtx::DAEMON,
                &sid,
                ("r1", "b1"),
                "https://clax.example/a/b1",
                "Theirs",
                true,
            )
            .unwrap()
        };
        let (m, w) = follow();
        assert!(w.replies_armed);
        assert!(
            !st.unfollow_remote(&m).unwrap(),
            "a live watch keeps it followed"
        );
        assert!(
            st.remote_link(&m, "r1")
                .unwrap()
                .unwrap()
                .unwatched_at
                .is_none()
        );
        let id = clax_core::ArtifactId::parse(&m).unwrap();
        st.unwatch(&clax_core::audit::AuditCtx::DAEMON, &sid, &id)
            .unwrap();
        assert!(st.unfollow_remote(&m).unwrap());
        clock.advance(7 * 24 * 3600 + 1);
        assert_eq!(st.mirrors_to_drop(st.now_utc()).unwrap(), std::slice::from_ref(&m));
        // Watched again before the drop: the drop leaves it.
        assert_eq!(follow().0, m);
        assert!(!st.drop_mirror(&m, st.now_utc()).unwrap());
        assert!(st.is_remote_mirror(&m).unwrap());
        st.unwatch(&clax_core::audit::AuditCtx::DAEMON, &sid, &id)
            .unwrap();
        assert!(st.unfollow_remote(&m).unwrap());
        assert!(
            !st.drop_mirror(&m, st.now_utc()).unwrap(),
            "unwatched for under a week"
        );
        clock.advance(7 * 24 * 3600 + 1);
        assert!(st.drop_mirror(&m, st.now_utc()).unwrap());
        assert_eq!(st.remote_mirror("r1", "b1").unwrap(), None);
    }
}
