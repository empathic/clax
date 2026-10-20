//! Commits pushed to an artifact's repository, recorded as versions (the
//! content module's [Pushes](clax_core::content#pushes)).
//!
//! Each pushed commit, oldest first, becomes the next version as a publish
//! would make it: under the artifact's content lock, the commit is made
//! version `n` in the content store, then one transaction checks the
//! current version and records `n` with the same authorship, visibility,
//! changelog and audit rules as a publish (`version.publish`), its
//! publisher the pusher; when the transaction fails, the commit stops
//! being version `n` and stays in git. A file the commit carries
//! unchanged from the version below keeps its authors; any other is the
//! pusher's. A publish records pushed commits before it writes, so a push
//! never blocks one, and a publish that did not see them is a conflict.
//!
//! The pusher of a commit is whoever made the push that left `main` at it,
//! or at the nearest commit above it that a push left `main` at
//! ([`clax_core::content::ContentStore::pushers`]); the caller's `fallback` for
//! commits no push is known for. Before a commit is recorded:
//!
//! - its pusher's rights are checked again, as they are at that moment: a
//!   connected person who is not revoked, whose rights allow `update` of
//!   `clax-artifacts` at the artifact; an agent whose session is live,
//!   whose effective rights (its person's, narrowed by its grant and
//!   session, with any elevation) allow it, and whose person is the one
//!   the token names; the owner and the owner's agents of this machine
//!   always. A commit whose pusher fails is not recorded nor tagged (it
//!   stays in git) and `version.push_refused` names it, once per commit;
//!   a later commit by someone who passes is recorded over it, and its
//!   note says which were left out. While `main` ends in refused commits a
//!   publish is refused (`push_refused`), since its commit would not
//!   follow `main`.
//! - every agent's token of the repository whose person may not see one of
//!   the new version's authors under the visibility filter is revoked (for
//!   a refused commit, which a clone still reaches, its pusher); when one
//!   cannot be, nothing more is recorded, and the next reconcile tries
//!   again.
//!
//! Publish's rules hold: a path [`check_path`] refuses, a path that
//! collides with one kept before it (the same name in another letter case,
//! or a file over a directory), a file over [`MAX_FILE_BYTES`], changed
//! files past [`MAX_BODY_BYTES`] in all, or past what the content store
//! reads of one commit ([`PushedContent::TooLarge`]), anything but a
//! regular file (a symbolic link, a submodule), and an `icon.svg` the
//! commit changes that breaks the icon rules ([`clax_core::icon::problem`])
//! are left out of the version (they stay in git), and its note names
//! them. An `icon.svg` the commit leaves as it was is kept unchecked, as
//! history holds it. A commit whose trees hold
//! more than [`clax_core::content::MAX_PUSHED_ENTRIES`] is refused as a commit
//! whose pusher may not publish is (`push_too_large`). The note is the commit's message,
//! after a line saying so when the commit does not simply follow the
//! version below it.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};

use super::Store;
use super::artifacts::VersionStep;
use crate::visibility::Visibility;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::content::{PushedCommit, PushedContent};
use clax_core::model::{Artifact, FileMeta, Version};
use clax_core::publish::{
    DecodedFile, FileChange, MAX_BODY_BYTES, MAX_FILE_BYTES, ValidatedPublish, check_path,
    content_type_for,
};
use clax_core::rights::RightType;
use clax_core::visibility::{Author, Reader, file_authors};
use clax_core::{ArtifactId, CoreError, Result};

pub use clax_core::content::Pusher;

/// The most paths a note names as left out.
const NAMED: usize = 10;

/// What one reconcile did.
#[derive(Debug, Default)]
pub(super) struct Reconciled {
    /// The versions recorded, oldest first.
    pub recorded: Vec<(Artifact, Version)>,
    /// The newest pushed commit, when it and the ones after the newest
    /// version are left out because their pushers may not publish.
    pub blocked: Option<String>,
}

/// Whether a pusher may publish the artifact now, and as whom.
#[derive(Debug, PartialEq, Eq)]
enum Standing {
    /// As this person (`None`: the owner) and session.
    Allowed {
        person: Option<String>,
        session: Option<String>,
    },
    /// Not: why, as `version.push_refused` records it.
    Refused(&'static str),
}

impl Store {
    /// Installs `f`, called with each version recorded from a pushed
    /// commit, after its transaction commits. Replaces any earlier one.
    pub fn set_pushed_listener(&self, f: impl Fn(&Artifact, &Version) + Send + Sync + 'static) {
        *self.pushed_listener.write().unwrap() = Some(std::sync::Arc::new(f));
    }

    /// Records the commits pushed to `id`'s repository past its current
    /// version as versions published by their pushers (`fallback` for
    /// commits the content store knows no pusher of), under `ctx`, and
    /// answers them oldest first (see the [module docs](self)). None for
    /// an artifact that is deleted, has no version, or whose content store
    /// takes no pushes. When a commit after the first cannot be recorded,
    /// the ones before it stay recorded and are answered; the rest wait
    /// for the next reconcile.
    pub async fn reconcile_pushes_async(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        fallback: &Pusher,
    ) -> Result<Vec<(Artifact, Version)>> {
        if self.get_artifact(id)?.is_none() {
            return Ok(Vec::new());
        }
        let _lock = self.lock_content(id).await?;
        Ok(self.record_pushes_locked(ctx, id, fallback).await?.recorded)
    }

    /// [`Store::reconcile_pushes_async`] while holding `id`'s content lock.
    pub(super) async fn record_pushes_locked(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        fallback: &Pusher,
    ) -> Result<Reconciled> {
        let mut out = Reconciled::default();
        let Some(a) = self.get_artifact(id)? else {
            return Ok(out);
        };
        let mut current = a.current_version;
        let pushed = self.content.pushed_commits(id, current).await?;
        if pushed.is_empty() {
            return Ok(out);
        }
        let commits: Vec<String> = pushed.iter().map(|c| c.commit.clone()).collect();
        let pushers = pushers_of(&self.content.pushers(id, &commits).await?, fallback);
        let mut refused: Vec<String> = Vec::new();
        for (c, pusher) in pushed.iter().zip(&pushers) {
            let (person, session) = match self.standing(id, pusher)? {
                Standing::Allowed { person, session } => (person, session),
                Standing::Refused(reason) => {
                    // Not a version, but on main where a clone reaches it.
                    let by = [Author::of_token(pusher.person.as_deref())];
                    self.revoke_blind_tokens(id, &by).await?;
                    self.record_push_refused(ctx, id, &c.commit, pusher, reason)?;
                    refused.push(c.commit.clone());
                    continue;
                }
            };
            match self
                .record_push(ctx, id, current, c, person, session.as_deref(), &refused)
                .await
            {
                // Its trees are past what is read of one commit: refused,
                // as a commit whose pusher may not publish is.
                Err(CoreError::Invalid {
                    code: "push_too_large",
                    ..
                }) => {
                    let by = [Author::of_token(pusher.person.as_deref())];
                    self.revoke_blind_tokens(id, &by).await?;
                    self.record_push_refused(ctx, id, &c.commit, pusher, "push_too_large")?;
                    refused.push(c.commit.clone());
                }
                Ok((a, v)) => {
                    current = v.n;
                    refused.clear();
                    let listen = self.pushed_listener.read().unwrap().clone();
                    if let Some(listen) = listen {
                        listen(&a, &v);
                    }
                    out.recorded.push((a, v));
                }
                Err(e) if out.recorded.is_empty() => return Err(e),
                Err(e) => {
                    tracing::warn!(
                        artifact = id.as_str(),
                        commit = c.commit.as_str(),
                        error = %e,
                        "could not record a pushed commit"
                    );
                    return Ok(out);
                }
            }
        }
        out.blocked = refused.pop();
        Ok(out)
    }

    /// Whether `pusher` may publish `id` now (see the [module docs](self)).
    fn standing(&self, id: &ArtifactId, pusher: &Pusher) -> Result<Standing> {
        let refused = |why| Ok(Standing::Refused(why));
        let person = match &pusher.person {
            None => None,
            Some(pid) => {
                let row: Option<(bool, bool, Option<String>)> = self.with_read(|c| {
                    Ok(c.query_row(
                        "SELECT person, owner, person_revoked_at FROM viewers WHERE public_id = ?1",
                        params![pid],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?)
                })?;
                match row {
                    None => return refused("unknown_person"),
                    Some((_, true, _)) => None,
                    Some((true, false, None)) => Some(pid.clone()),
                    Some((true, false, Some(_))) => return refused("person_revoked"),
                    Some((false, false, _)) => return refused("not_a_person"),
                }
            }
        };
        let may = |rights: &clax_core::rights::Rights, person: Option<&str>| {
            let own = |x: &str| self.artifact_created_by(x, person).unwrap_or(false);
            rights.allows(RightType::Artifacts, "update", Some(id.as_str()), &own)
        };
        let Some(sid) = &pusher.session_id else {
            return match &person {
                None => Ok(Standing::Allowed {
                    person: None,
                    session: None,
                }),
                Some(pid) if may(&self.person_rights(pid)?, Some(pid)) => Ok(Standing::Allowed {
                    person,
                    session: None,
                }),
                Some(_) => refused("no_right"),
            };
        };
        let Some(agent) = self.agent_rights(sid)? else {
            return refused("session_ended");
        };
        if agent.session.person != person {
            return refused("foreign_session");
        }
        let allowed = match (&agent.session.grant_id, &agent.rights) {
            // This machine's agent, acting with the daemon token.
            (None, _) => person.is_none(),
            (Some(_), None) => return refused("grant_ended"),
            (Some(_), Some(r)) => may(r, person.as_deref()),
        };
        if allowed {
            Ok(Standing::Allowed {
                person,
                session: Some(sid.clone()),
            })
        } else {
            refused("no_right")
        }
    }

    /// Records `version.push_refused` for `commit` of `id`, pushed by
    /// `pusher`, unless an earlier one names it.
    fn record_push_refused(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        commit: &str,
        pusher: &Pusher,
        reason: &'static str,
    ) -> Result<()> {
        tracing::warn!(
            artifact = id.as_str(),
            commit,
            person = pusher.person.as_deref(),
            session = pusher.session_id.as_deref(),
            reason,
            "a pushed commit's pusher may not publish here: it is not recorded"
        );
        self.with_tx(|tx| {
            let seen: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM audit_events
                 WHERE artifact_id = ?1 AND kind = ?2 AND json_extract(body, '$.commit') = ?3)",
                params![id.as_str(), AuditKind::VersionPushRefused.as_str(), commit],
                |r| r.get(0),
            )?;
            if seen {
                return Ok(());
            }
            let mut rec = AuditRecord::new(AuditKind::VersionPushRefused, self.now())
                .with("commit", commit)
                .with("person", pusher.person.clone())
                .with("session", pusher.session_id.clone())
                .with("reason", reason);
            rec.ids.artifact = Some(id.as_str().to_string());
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }

    /// Revokes every agent's token of `id`'s repository whose person may
    /// not see all of `authors` there (see the [module docs](self)); a
    /// publish calls it with its publisher before its commit is written.
    pub(super) async fn revoke_blind_tokens(
        &self,
        id: &ArtifactId,
        authors: &[Author],
    ) -> Result<()> {
        let tokens = self.content.repo_tokens(id).await?;
        let generation = self.visibility_generation();
        for t in tokens {
            let Some(sid) = &t.holder.session_id else {
                continue;
            };
            let person = match self.get_session(sid)? {
                Some(s) => s.person,
                None => t.holder.person.clone(),
            };
            let reader = person.map_or(Reader::Owner, Reader::Person);
            let vis = Visibility::for_reader(self, &reader, generation)?;
            if vis.sees(authors, id.as_str(), None) {
                continue;
            }
            self.content.revoke_repo_token(id, &t.id).await?;
            tracing::info!(
                artifact = id.as_str(),
                token = t.id.as_str(),
                session = sid.as_str(),
                "revoked an agent's repository token whose person may not see a pushed version"
            );
        }
        Ok(())
    }

    /// Records pushed commit `c` as version `current + 1`; `refused` are
    /// the commits pushed after `current` and before `c` that were left out.
    #[allow(clippy::too_many_arguments)]
    async fn record_push(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        current: u32,
        c: &PushedCommit,
        person: Option<String>,
        session: Option<&str>,
        refused: &[String],
    ) -> Result<(Artifact, Version)> {
        let below = self.get_version(id, current)?;
        let prev: BTreeMap<String, FileMeta> =
            below.as_ref().map(|v| v.files.clone()).unwrap_or_default();
        let mut files = self.content.pushed_files(id, &c.commit, current).await?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut changes: BTreeMap<String, FileChange> = BTreeMap::new();
        let mut kept = KeptPaths::default();
        let mut skipped: Vec<String> = Vec::new();
        // A pushed icon.svg that breaks the icon rules, which every
        // publish runs: left out as a file over the limits is.
        let mut bad_icon = None;
        let mut total: u64 = 0;
        for f in files {
            if check_path(&f.path).is_err() || kept.collides(&f.path) {
                skipped.push(f.path);
                continue;
            }
            match f.content {
                PushedContent::Same if prev.contains_key(&f.path) => kept.keep(f.path),
                // Left out of the version below as well.
                PushedContent::Same | PushedContent::NotAFile | PushedContent::TooLarge => {
                    skipped.push(f.path)
                }
                PushedContent::Bytes(bytes) => {
                    let size = bytes.len() as u64;
                    if size > MAX_FILE_BYTES || total + size > MAX_BODY_BYTES {
                        skipped.push(f.path);
                        continue;
                    }
                    if f.path == clax_core::icon::ICON_PATH
                        && let Some(p) = clax_core::icon::problem(&bytes)
                    {
                        bad_icon = Some(p);
                        continue;
                    }
                    total += size;
                    let content_type = content_type_for(&f.path);
                    kept.keep(f.path.clone());
                    changes.insert(
                        f.path,
                        FileChange::Put(DecodedFile {
                            bytes,
                            content_type,
                        }),
                    );
                }
            }
        }
        for path in prev.keys() {
            if !kept.contains(path) {
                changes.insert(path.clone(), FileChange::Remove);
            }
        }
        // The new version's authors: its publisher, and whoever wrote the
        // files it carries unchanged.
        let mut authors = vec![Author::of_token(person.as_deref())];
        if let Some(v) = &below {
            for path in kept.paths.iter().filter(|p| !changes.contains_key(*p)) {
                for a in file_authors(&v.authors, path) {
                    if !authors.contains(&a) {
                        authors.push(a);
                    }
                }
            }
        }
        self.revoke_blind_tokens(id, &authors).await?;
        let (note, note_truncated) = clax_core::working::clean_line(
            &push_note(c, current, &skipped, bad_icon.as_ref(), refused),
            clax_core::changelog::MAX_NOTE_CHARS,
        );
        let p = ValidatedPublish {
            title: None,
            description: None,
            icon: None,
            label: None,
            if_version: Some(current),
            capabilities: None,
            files: changes,
            note,
            note_truncated,
            addresses: Vec::new(),
            working_threads: Vec::new(),
            person,
            by_page: false,
            hidden: Vec::new(),
        };
        self.record_version_locked(
            ctx,
            id,
            current,
            &p,
            &prev,
            session,
            VersionStep::Adopt(&c.commit),
            |_, _| Ok(()),
        )
        .await
        .map(|(a, v, ())| (a, v))
    }
}

/// The paths a pushed version keeps, in path order, and what tells whether
/// a later path collides with one of them (`artifacts::collide`) without comparing
/// it with each.
#[derive(Default)]
struct KeptPaths {
    paths: std::collections::BTreeSet<String>,
    /// Each kept path, in lowercase.
    files: std::collections::HashSet<String>,
    /// Each directory above a kept path, in lowercase.
    dirs: std::collections::HashSet<String>,
}

impl KeptPaths {
    fn keep(&mut self, path: String) {
        let lower = path.to_ascii_lowercase();
        for (i, _) in lower.match_indices('/') {
            self.dirs.insert(lower[..i].to_string());
        }
        self.files.insert(lower);
        self.paths.insert(path);
    }

    fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    /// Whether `path` collides with a kept path: the same in another
    /// letter case, a directory above one, or below one.
    fn collides(&self, path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        self.files.contains(&lower)
            || self.dirs.contains(&lower)
            || lower
                .match_indices('/')
                .any(|(i, _)| self.files.contains(&lower[..i]))
    }
}

/// The pusher of each commit, oldest first, from what the content store
/// `named` (whoever left `main` at it): a commit no push left `main` at was
/// pushed with the nearest commit above it that one did, and a commit
/// with none above it by `fallback`.
fn pushers_of(named: &[Option<Pusher>], fallback: &Pusher) -> Vec<Pusher> {
    let mut out = vec![fallback.clone(); named.len()];
    let mut above: Option<&Pusher> = None;
    for (i, n) in named.iter().enumerate().rev() {
        if let Some(p) = n {
            above = Some(p);
        }
        if let Some(p) = above {
            out[i] = p.clone();
        }
    }
    out
}

/// The error of a publish while `main` ends in commit `commit`, pushed by
/// someone who may not publish the artifact.
pub(super) fn push_refused(commit: &str) -> CoreError {
    let short = &commit[..commit.len().min(12)];
    CoreError::invalid(
        "push_refused",
        format!(
            "main ends in commit {short}, pushed by someone who may not publish here, so it is not a version and a publish cannot follow it: someone who may publish here pushes a commit over it"
        ),
    )
}

/// The note of the version made from `c`, pushed past version `prev`:
/// lines saying how it relates to `prev` when it does not simply follow
/// it (`refused`: the commits between them left out because whoever
/// pushed them may not publish), which files were left out and the icon
/// rule a left-out `icon.svg` breaks, then its message.
fn push_note(
    c: &PushedCommit,
    prev: u32,
    skipped: &[String],
    bad_icon: Option<&clax_core::icon::IconProblem>,
    refused: &[String],
) -> String {
    let short = &c.commit[..c.commit.len().min(12)];
    let mut lead: Vec<String> = Vec::new();
    if !refused.is_empty() {
        let shorts: Vec<&str> = refused
            .iter()
            .take(NAMED)
            .map(|r| &r[..r.len().min(12)])
            .collect();
        let more = if refused.len() > NAMED { ", …" } else { "" };
        lead.push(format!(
            "Not versions (pushed by someone who may not publish here; {short} carries their changes): {}{more}.",
            shorts.join(", ")
        ));
    } else if c.collapsed {
        lead.push(format!(
            "main's head {short} stands for every commit pushed after v{prev}."
        ));
    } else if !c.follows {
        lead.push(format!(
            "History was rewritten: {short} does not follow v{prev}."
        ));
    }
    if !skipped.is_empty() {
        let shown: Vec<&str> = skipped.iter().take(NAMED).map(String::as_str).collect();
        let more = if skipped.len() > NAMED { ", …" } else { "" };
        let count = if skipped.len() == 1 {
            "1 file was".to_string()
        } else {
            format!("{} files were", skipped.len())
        };
        lead.push(format!(
            "{count} not taken (over Clax's path or size limits, or not a regular file; still in git): {}{more}.",
            shown.join(", ")
        ));
    }
    if let Some(p) = bad_icon {
        lead.push(format!(
            "{} was not taken (it breaks the icon rules: {}; still in git).",
            clax_core::icon::ICON_PATH,
            p.code()
        ));
    }
    let message = c.message.trim();
    match (lead.is_empty(), message.is_empty()) {
        (true, _) => message.to_string(),
        (false, true) => lead.join(" "),
        (false, false) => format!("{} {message}", lead.join(" ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(follows: bool, collapsed: bool) -> PushedCommit {
        PushedCommit {
            commit: "0123456789abcdef0123".into(),
            message: "Fix the header\n\nLonger body".into(),
            follows,
            collapsed,
        }
    }

    #[test]
    fn a_push_note_is_the_message_after_what_was_left_out_or_rewritten() {
        assert_eq!(
            push_note(&commit(true, false), 3, &[], None, &[]),
            "Fix the header\n\nLonger body"
        );
        assert_eq!(
            push_note(&commit(false, false), 3, &["a\tb".into()], None, &[]),
            "History was rewritten: 0123456789ab does not follow v3. 1 file was not taken (over Clax's path or size limits, or not a regular file; still in git): a\tb. Fix the header\n\nLonger body"
        );
        assert!(
            push_note(&commit(true, true), 3, &[], None, &[])
                .starts_with("main's head 0123456789ab stands for every commit pushed after v3.")
        );
        let many: Vec<String> = (0..12).map(|i| format!("f{i}")).collect();
        assert!(
            push_note(&commit(true, false), 1, &many, None, &[])
                .starts_with("12 files were not taken")
        );
        assert!(
            push_note(&commit(false, false), 3, &[], None, &["fedcba9876543210".into()]).starts_with(
                "Not versions (pushed by someone who may not publish here; 0123456789ab carries their changes): fedcba987654."
            )
        );
    }

    #[test]
    fn a_commit_is_pushed_with_the_nearest_commit_above_it_a_push_left_main_at() {
        let p = |s: &str| Pusher {
            person: Some(s.into()),
            session_id: None,
        };
        let fallback = Pusher::default();
        assert_eq!(
            pushers_of(
                &[None, Some(p("rae")), None, None, Some(p("sam"))],
                &fallback
            ),
            [p("rae"), p("rae"), p("sam"), p("sam"), p("sam")]
        );
        assert_eq!(
            pushers_of(&[Some(p("rae")), None], &fallback),
            [p("rae"), fallback.clone()],
            "a commit above every known push is the fallback's"
        );
        assert!(pushers_of(&[], &fallback).is_empty());
    }
}
