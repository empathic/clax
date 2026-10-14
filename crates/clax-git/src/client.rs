//! A git smart-HTTP receive-pack client (protocol v0), how Clax writes
//! commits and tags to a remote whose API has no write call (Cloudflare
//! Artifacts): it reads the ref advertisement, sends ref update commands
//! with a pack of the new objects, and reads `report-status`. Auth is
//! `Authorization: Bearer <token>`.
//!
//! The request builders and response parsers are pure; [`apply`],
//! [`set_refs`] and [`delete_refs`] drive them over a caller's
//! [`Transport`].

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{GitError, Result};
use crate::http::{HttpRequest, HttpResponse, Transport};
use crate::id::ObjectId;
use crate::object::Object;
use crate::pack::write_pack;
use crate::pktline::{self, FLUSH};
use crate::repo::{ATOMIC_FAILED, RefStatus, RefUpdate};

/// A remote repository and the token to reach it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    /// The repository URL, such as `https://host/git/a-0123456789ab.git`.
    pub url: String,
    pub token: String,
}

impl Remote {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Remote {
        Remote {
            url: url.into(),
            token: token.into(),
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{path}", self.url.trim_end_matches('/'))
    }

    /// `GET <remote>/info/refs?service=git-receive-pack`.
    pub fn advertisement_request(&self) -> HttpRequest {
        HttpRequest {
            method: "GET",
            url: self.endpoint("info/refs?service=git-receive-pack"),
            headers: vec![
                ("authorization", format!("Bearer {}", self.token)),
                ("git-protocol", "version=0".into()),
            ],
            body: Vec::new(),
        }
    }

    /// `POST <remote>/git-receive-pack` with `updates` and, unless every
    /// update is a delete, a pack of `objects`. Asks for `report-status`,
    /// and for `atomic` when `atomic` is set, the remote offers it and there
    /// is more than one update.
    pub fn receive_pack_request(
        &self,
        updates: &[RefUpdate],
        objects: &[Object],
        capabilities: &BTreeSet<String>,
        atomic: bool,
    ) -> HttpRequest {
        HttpRequest {
            method: "POST",
            url: self.endpoint("git-receive-pack"),
            headers: vec![
                ("authorization", format!("Bearer {}", self.token)),
                (
                    "content-type",
                    "application/x-git-receive-pack-request".into(),
                ),
                ("accept", "application/x-git-receive-pack-result".into()),
            ],
            body: receive_pack_body(updates, objects, capabilities, atomic),
        }
    }
}

/// The body of a receive-pack request (see [`Remote::receive_pack_request`]).
pub fn receive_pack_body(
    updates: &[RefUpdate],
    objects: &[Object],
    capabilities: &BTreeSet<String>,
    atomic: bool,
) -> Vec<u8> {
    let mut caps = vec!["report-status"];
    if atomic && updates.len() > 1 && capabilities.contains("atomic") {
        caps.push("atomic");
    }
    let mut out = Vec::new();
    for (i, u) in updates.iter().enumerate() {
        let mut line = format!("{} {} {}", u.old, u.new, u.name);
        if i == 0 {
            line.push('\0');
            line.push_str(&caps.join(" "));
        }
        line.push('\n');
        pktline::write(&mut out, line.as_bytes());
    }
    out.extend_from_slice(FLUSH);
    if updates.iter().any(|u| !u.is_delete()) {
        out.extend_from_slice(&write_pack(objects));
    }
    out
}

/// A remote's refs and capabilities, from its advertisement.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Advertisement {
    pub refs: BTreeMap<String, ObjectId>,
    pub capabilities: BTreeSet<String>,
}

/// The body of a response with a success status, or the error its status
/// means: 404 `not_found`, 401 and 403 [`GitError::Auth`], any other
/// failure `repo_unavailable`.
pub fn checked(response: &HttpResponse, what: &str) -> Result<()> {
    match response.status {
        200..=299 => Ok(()),
        404 => Err(GitError::not_found(format!(
            "The git remote has no such repository ({what})"
        ))),
        401 | 403 => Err(GitError::Auth {
            status: response.status,
            message: format!(
                "The git remote refused the token ({}) on {what}",
                response.status
            ),
        }),
        s => {
            let head = &response.body[..response.body.len().min(200)];
            Err(GitError::unavailable(format!(
                "The git remote answered {s} to {what}: {}",
                String::from_utf8_lossy(head)
            )))
        }
    }
}

/// Parses a v0 ref advertisement (either service's).
pub fn parse_advertisement(response: &HttpResponse) -> Result<Advertisement> {
    checked(response, "info/refs")?;
    let mut adv = Advertisement::default();
    let mut first = true;
    for pkt in pktline::read_all(&response.body)? {
        let Some(mut line) = pkt.text() else { continue };
        if line.starts_with('#') || line == "version 1" {
            continue;
        }
        if let Some(nul) = line.find('\0') {
            if first {
                adv.capabilities.extend(
                    line[nul + 1..]
                        .split(' ')
                        .filter(|c| !c.is_empty())
                        .map(String::from),
                );
            }
            line.truncate(nul);
        }
        first = false;
        let Some((hash, name)) = line.split_once(' ') else {
            continue;
        };
        if name == "capabilities^{}" {
            continue;
        }
        let id = ObjectId::from_hex(hash)
            .ok_or_else(|| GitError::unavailable("A malformed ref advertisement"))?;
        adv.refs.insert(name.to_string(), id);
    }
    Ok(adv)
}

/// A receive-pack `report-status`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PushReport {
    /// `ok`, or the reason unpacking failed; empty when the remote sent none.
    pub unpack: String,
    pub refs: BTreeMap<String, RefStatus>,
}

/// Parses a receive-pack response.
pub fn parse_report(response: &HttpResponse) -> Result<PushReport> {
    checked(response, "git-receive-pack")?;
    let mut report = PushReport::default();
    for pkt in pktline::read_all(&response.body)? {
        let Some(line) = pkt.text() else { continue };
        if let Some(rest) = line.strip_prefix("unpack ") {
            report.unpack = rest.to_string();
        } else if let Some(name) = line.strip_prefix("ok ") {
            report.refs.insert(name.to_string(), Ok(()));
        } else if let Some(rest) = line.strip_prefix("ng ") {
            let (name, reason) = rest.split_once(' ').unwrap_or((rest, ""));
            report
                .refs
                .insert(name.to_string(), Err(reason.to_string()));
        }
    }
    Ok(report)
}

/// The remote's ref advertisement.
pub async fn advertised_refs(remote: &Remote, transport: &impl Transport) -> Result<Advertisement> {
    parse_advertisement(&transport.send(remote.advertisement_request()).await?)
}

async fn post(
    remote: &Remote,
    transport: &impl Transport,
    updates: &[RefUpdate],
    objects: &[Object],
    adv: &Advertisement,
    atomic: bool,
) -> Result<Vec<RefStatus>> {
    let request = remote.receive_pack_request(updates, objects, &adv.capabilities, atomic);
    let report = parse_report(&transport.send(request).await?)?;
    if report.unpack != "ok" {
        let why = if report.unpack.is_empty() {
            "no report"
        } else {
            &report.unpack
        };
        return Err(GitError::unavailable(format!(
            "The git remote could not unpack the push: {why}"
        )));
    }
    updates
        .iter()
        .map(|u| {
            report.refs.get(&u.name).cloned().ok_or_else(|| {
                GitError::unavailable(format!("The git remote did not report on {}", u.name))
            })
        })
        .collect()
}

/// Pushes `objects` and applies `updates` on the remote, with the
/// semantics of [`crate::repo::Repo::apply`]: an update whose `old` the
/// advertisement contradicts is refused (`stale info`) without sending
/// anything for it. With `atomic`, a remote that offers `atomic` takes every
/// update in one request or none; one that does not is sent each update in
/// its own request, in order (the first carrying the pack), stopping at the
/// first refusal (the rest answer [`ATOMIC_FAILED`]), so an earlier update
/// may stand while a later one is refused. One advertisement read, then
/// one request (or one per update), so a publish costs a GET and a POST.
pub async fn apply(
    remote: &Remote,
    transport: &impl Transport,
    objects: &[Object],
    updates: &[RefUpdate],
    atomic: bool,
) -> Result<Vec<RefStatus>> {
    if updates.is_empty() {
        return Ok(Vec::new());
    }
    let adv = advertised_refs(remote, transport).await?;
    let stale: Vec<bool> = updates
        .iter()
        .map(|u| adv.refs.get(&u.name).copied().unwrap_or(ObjectId::ZERO) != u.old)
        .collect();
    if atomic && stale.iter().any(|s| *s) {
        return Ok(stale
            .iter()
            .map(|s| Err(if *s { "stale info" } else { ATOMIC_FAILED }.to_string()))
            .collect());
    }
    if updates.iter().any(|u| u.is_delete()) && !adv.capabilities.contains("delete-refs") {
        return Err(GitError::unavailable(
            "The git remote does not accept ref deletions",
        ));
    }
    if (atomic && adv.capabilities.contains("atomic")) || updates.len() == 1 {
        return post(remote, transport, updates, objects, &adv, atomic).await;
    }
    let mut out = Vec::with_capacity(updates.len());
    let mut sent_objects = false;
    for (u, stale) in updates.iter().zip(stale) {
        if stale {
            out.push(Err("stale info".to_string()));
            continue;
        }
        // Asked for all or nothing: what follows a refusal is not sent.
        if atomic && out.iter().any(|s: &RefStatus| s.is_err()) {
            out.push(Err(ATOMIC_FAILED.to_string()));
            continue;
        }
        let objs: &[Object] = if sent_objects || u.is_delete() {
            &[]
        } else {
            objects
        };
        let status = post(
            remote,
            transport,
            std::slice::from_ref(u),
            objs,
            &adv,
            false,
        )
        .await;
        // A failure after an earlier update stood is that update's status,
        // so the caller still learns what moved.
        let status = match status {
            Ok(mut s) => s.remove(0),
            Err(e) if !out.is_empty() && out.iter().any(|s: &RefStatus| s.is_ok()) => {
                Err(e.to_string())
            }
            Err(e) => return Err(e),
        };
        if status.is_ok() && !u.is_delete() {
            sent_objects = true;
        }
        out.push(status);
    }
    Ok(out)
}

/// Sets each of `refs` to its value in one receive-pack request: an ID
/// creates or moves the ref (with an empty pack: the object must already be
/// on the remote), `None` deletes it. Refs already at their value, and
/// deletions of refs the remote lacks, are skipped. Answers the refs
/// changed; a refusal is `repo_unavailable`.
pub async fn set_refs(
    remote: &Remote,
    transport: &impl Transport,
    refs: &BTreeMap<String, Option<ObjectId>>,
) -> Result<Vec<String>> {
    let adv = advertised_refs(remote, transport).await?;
    let updates: Vec<RefUpdate> = refs
        .iter()
        .filter_map(|(name, want)| {
            let at = adv.refs.get(name).copied().unwrap_or(ObjectId::ZERO);
            let want = want.unwrap_or(ObjectId::ZERO);
            (at != want).then(|| RefUpdate {
                name: name.clone(),
                old: at,
                new: want,
            })
        })
        .collect();
    if updates.is_empty() {
        return Ok(Vec::new());
    }
    if updates.iter().any(|u| u.is_delete()) && !adv.capabilities.contains("delete-refs") {
        return Err(GitError::unavailable(
            "The git remote does not accept ref deletions",
        ));
    }
    let statuses = post(remote, transport, &updates, &[], &adv, false).await?;
    for (u, s) in updates.iter().zip(statuses) {
        if let Err(reason) = s {
            return Err(GitError::unavailable(format!(
                "The git remote refused to update {}: {reason}",
                u.name
            )));
        }
    }
    Ok(updates.into_iter().map(|u| u.name).collect())
}

/// Deletes each of `refs` the remote has in one request; refs it does not
/// advertise are skipped. Answers the refs deleted.
pub async fn delete_refs(
    remote: &Remote,
    transport: &impl Transport,
    refs: &[String],
) -> Result<Vec<String>> {
    let map = refs.iter().map(|r| (r.clone(), None)).collect();
    set_refs(remote, transport, &map).await
}

/// Whether a refusal reason means another writer moved the ref first.
pub fn is_conflict_reason(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    [
        "non-fast-forward",
        "fetch first",
        "stale",
        "lock",
        "failed to update",
        "already exists",
    ]
    .iter()
    .any(|w| r.contains(w))
}

/// Whether a refusal reason is the one an atomic push gives the refs that
/// did not fail themselves.
pub fn is_atomic_collateral(reason: &str) -> bool {
    reason.to_ascii_lowercase().starts_with("atomic push fail")
}

/// Reads objects of a repository whose writes go over receive-pack, such
/// as a Cloudflare Artifacts repository read through its binding.
#[allow(async_fn_in_trait)]
pub trait ObjectReader {
    async fn read_object(&self, id: &ObjectId) -> Result<Option<Object>>;
}

/// A [`crate::repo::Repo`] that reads objects through `reader`, reads refs
/// from the remote's receive-pack advertisement, and writes with [`apply`].
pub struct PushingRepo<R, T> {
    pub reader: R,
    pub remote: Remote,
    pub transport: T,
}

impl<R: ObjectReader, T: Transport> crate::repo::Repo for PushingRepo<R, T> {
    async fn read_object(&self, id: &ObjectId) -> Result<Option<Object>> {
        self.reader.read_object(id).await
    }

    async fn refs(&self) -> Result<BTreeMap<String, ObjectId>> {
        Ok(advertised_refs(&self.remote, &self.transport).await?.refs)
    }

    async fn apply(
        &self,
        objects: Vec<Object>,
        updates: &[RefUpdate],
        atomic: bool,
    ) -> Result<Vec<RefStatus>> {
        apply(&self.remote, &self.transport, &objects, updates, atomic).await
    }
}
