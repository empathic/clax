//! The server side of git smart HTTP (protocol v0) over a caller's
//! [`Repo`]: `git clone` and `git fetch` (upload-pack) and `git push`
//! (receive-pack), so a local stand-in store serves what Cloudflare
//! Artifacts serves.
//!
//! Upload-pack:
//! - Version 2 is not spoken: a client's `Git-Protocol: version=2` is
//!   ignored and the v0 advertisement answers, which git and jj (through
//!   git) accept as a downgrade.
//! - Capabilities advertised: `multi_ack_detailed`, `side-band`,
//!   `side-band-64k`, `no-progress`, `symref=HEAD:<head>`,
//!   `object-format=sha1`, `agent`. No `shallow`, `no-done`, `thin-pack`,
//!   `ofs-delta`, `include-tag` or `filter`.
//! - Negotiation follows git's upload-pack in stateless (HTTP) mode: a have
//!   that is a commit of this repository is common (`ACK <id> common`, or
//!   `ACK <id> ready` once every want descends from a common commit); any
//!   other have is never read. A round ends with `NAK`; `done` ends with
//!   `ACK <last common>`, or `NAK` when nothing is common. A client that
//!   does not ask for `multi_ack_detailed` gets `NAK` for every round.
//! - A want must be a ref's value or a commit one reaches (a ref may move
//!   between the requests of one fetch).
//! - The pack holds every object reachable from the wants, less the commits
//!   the common haves reach and the trees and files of the common haves
//!   themselves; objects are stored whole, undeltified.
//!
//! Receive-pack:
//! - Capabilities advertised: `report-status`, `delete-refs`, `atomic`,
//!   `ofs-delta`, `object-format=sha1`, `agent`. Packs may be thin and
//!   deltified; delta bases outside the pack are read from the repository.
//! - Each update is checked before any applies: a well-formed ref name
//!   under `refs/`, every object the new value reaches present and of the
//!   type that names it (a commit's tree a tree, …), a fast
//!   forward for `refs/heads/*` (unless allowed), no deletion of the branch
//!   `HEAD` names, and no creating, moving or deleting `refs/tags/v<n>`:
//!   Clax writes its version tags `v<n>` itself, with [`Repo::apply`], and
//!   a push may not touch them (unless allowed). Then [`Repo::apply`]
//!   compares and swaps each ref.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::error::{GitError, Result};
use crate::http::HttpResponse;
use crate::id::ObjectId;
use crate::object::{Commit, Mode, Object, ObjectKind, parse_tree, tag_target};
use crate::pack::{
    DEFAULT_MAX_OBJECT_BYTES, DEFAULT_MAX_UNPACKED_BYTES, ParsedPack, Resolved, write_pack,
};
use crate::pktline::{self, FLUSH, Packet};
use crate::repo::{ATOMIC_FAILED, RefStatus, RefUpdate, Repo};

const AGENT: &str = "agent=clax";
const SIDE_BAND_64K_PAYLOAD: usize = 65516;
const SIDE_BAND_PAYLOAD: usize = 1000 - 4;
/// The most want and have lines one upload-pack request may carry.
pub const MAX_LINES: usize = 65536;

/// The prefix of the refs [`ReceiveOptions::protect_version_tags`] keeps
/// from pushes: `refs/tags/v` followed by digits alone
/// ([`is_version_tag`]).
pub const VERSION_TAGS: &str = "refs/tags/v";

/// Why receive-pack refuses an update of a protected version tag.
pub const VERSION_TAG_REFUSED: &str = "refs/tags/v<n> are Clax's version tags";

/// Whether `name` is a ref [`ReceiveOptions::protect_version_tags`] keeps
/// from pushes: exactly `refs/tags/v<digits>` (leading zeros included, so
/// no look-alike of a version tag is taken either). A pusher's own tags
/// that merely start with `v`, such as `v1.0` or `vendor`, are not.
pub fn is_version_tag(name: &str) -> bool {
    name.strip_prefix(VERSION_TAGS)
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

/// What the caller's authentication grants on the repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Clone and fetch.
    Read,
    /// Clone, fetch and push.
    Write,
}

/// Limits and policy for [`serve`].
#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// The largest upload-pack request body, as sent (default 1 MiB).
    pub max_upload_request_bytes: usize,
    /// The largest upload-pack request body once gzip-inflated (default 8 MiB).
    pub max_inflated_bytes: usize,
    /// The largest receive-pack request body (default 64 MiB).
    pub max_push_bytes: usize,
    pub receive: ReceiveOptions,
}

impl Default for ServeOptions {
    fn default() -> Self {
        ServeOptions {
            max_upload_request_bytes: 1024 * 1024,
            max_inflated_bytes: 8 * 1024 * 1024,
            max_push_bytes: 64 * 1024 * 1024,
            receive: ReceiveOptions::default(),
        }
    }
}

impl ServeOptions {
    /// The most body bytes the caller should read for a request to `rest`
    /// (see [`GitRequest::rest`]) before answering 413 itself.
    pub fn body_limit(&self, rest: &str) -> usize {
        if rest == "git-receive-pack" {
            self.max_push_bytes
        } else {
            self.max_upload_request_bytes
        }
    }
}

/// Receive-pack policy.
#[derive(Clone, Debug)]
pub struct ReceiveOptions {
    /// Refuse an update of a `refs/heads/*` ref to a commit that does not
    /// descend from its old value (default true: versions on `main` only
    /// append).
    pub deny_non_fast_forwards: bool,
    /// Refuse every ref deletion (default false; deleting the branch `HEAD`
    /// names is always refused).
    pub deny_deletes: bool,
    /// Refuse every update of a version tag, [`is_version_tag`] (default true:
    /// Clax owns its version tags `v<n>`; a push that created `v<n+1>`
    /// first would make the next publish fail, and one that moved `v<n>`
    /// would have the version read another commit than the one recorded).
    pub protect_version_tags: bool,
    /// The most bytes one pushed object may hold.
    pub max_object_bytes: usize,
    /// The most bytes a push's pack may unpack to: its entries inflated
    /// plus the objects its deltas make.
    pub max_unpacked_bytes: usize,
}

impl Default for ReceiveOptions {
    fn default() -> Self {
        ReceiveOptions {
            deny_non_fast_forwards: true,
            deny_deletes: false,
            protect_version_tags: true,
            max_object_bytes: DEFAULT_MAX_OBJECT_BYTES,
            max_unpacked_bytes: DEFAULT_MAX_UNPACKED_BYTES,
        }
    }
}

// ── HTTP ────────────────────────────────────────────────────────────────────

/// One smart-HTTP request, with the repository already chosen by the
/// caller's routing.
#[derive(Clone, Debug)]
pub struct GitRequest<'a> {
    /// `GET`, `HEAD` or `POST`.
    pub method: &'a str,
    /// What follows `<repo>.git/`: `info/refs`, `git-upload-pack` or
    /// `git-receive-pack`.
    pub rest: &'a str,
    /// The query string without `?`, such as `service=git-upload-pack`.
    pub query: Option<&'a str>,
    /// The `Content-Encoding` header.
    pub content_encoding: Option<&'a str>,
    pub body: &'a [u8],
}

/// What [`serve`] answers: the response, and the ref updates a push
/// applied (empty for anything else), for the caller to act on (record
/// versions, send a pushed event).
#[derive(Clone, Debug)]
pub struct Served {
    pub response: HttpResponse,
    pub pushed: Vec<RefUpdate>,
}

/// Splits `/git/<repo>.git/<rest>` into the repository name and `rest`
/// when `rest` is a smart-HTTP endpoint (`info/refs`, `git-upload-pack`,
/// `git-receive-pack`).
pub fn split_git_path(path: &str) -> Option<(&str, &str)> {
    let tail = path.strip_prefix("/git/")?;
    let (repo, rest) = tail.split_once(".git/")?;
    if repo.is_empty() || repo.contains('/') {
        return None;
    }
    matches!(rest, "info/refs" | "git-upload-pack" | "git-receive-pack").then_some((repo, rest))
}

/// The token of an `Authorization: Bearer <token>` header (the scheme in
/// any case).
pub fn bearer_token(header: &str) -> Option<&str> {
    let h = header.trim();
    let (scheme, token) = h.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim_start_matches(' ');
    let ok = !token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~+/=-".contains(&b));
    ok.then_some(token)
}

fn text(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![
            ("content-type".into(), "text/plain; charset=utf-8".into()),
            ("cache-control".into(), "no-store".into()),
        ],
        body: body.as_bytes().to_vec(),
    }
}

/// The 401 to answer a request without a token good for the repository
/// (whether or not the repository exists).
pub fn unauthorized() -> HttpResponse {
    let mut r = text(
        401,
        "a git token for this repository is required: Authorization: Bearer <token>\n",
    );
    r.headers
        .push(("www-authenticate".into(), "Bearer realm=\"clax\"".into()));
    r
}

/// A 413 for a body over `limit` bytes.
pub fn too_large(limit: usize) -> HttpResponse {
    text(413, &format!("the request body is over {limit} bytes\n"))
}

fn binary(content_type: &str, body: Vec<u8>) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![
            ("content-type".into(), content_type.into()),
            ("cache-control".into(), "no-cache".into()),
        ],
        body,
    }
}

fn query_service(query: Option<&str>) -> Option<&str> {
    query?.split('&').find_map(|kv| kv.strip_prefix("service="))
}

/// Inflates a gzip body, or `None` past `limit` bytes.
fn gunzip(raw: &[u8], limit: usize) -> std::result::Result<Vec<u8>, &'static str> {
    if raw.len() < 18 || raw[0] != 0x1f || raw[1] != 0x8b || raw[2] != 8 {
        return Err("not gzip");
    }
    let flags = raw[3];
    let mut at = 10;
    if flags & 4 != 0 {
        let n = u16::from_le_bytes([raw[at], raw[at + 1]]) as usize;
        at += 2 + n;
    }
    for bit in [8, 16] {
        if flags & bit != 0 {
            while at < raw.len() && raw[at] != 0 {
                at += 1;
            }
            at += 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    let deflated = raw.get(at..).ok_or("truncated gzip")?;
    miniz_oxide::inflate::decompress_to_vec_with_limit(deflated, limit).map_err(|e| {
        match e.status {
            miniz_oxide::inflate::TINFLStatus::HasMoreOutput => "too large",
            _ => "bad gzip",
        }
    })
}

/// Answers one smart-HTTP request on `repo` for a caller granted `access`.
/// Authentication, routing to the repository and reading the body (at most
/// [`ServeOptions::body_limit`] bytes) are the caller's.
pub async fn serve(
    repo: &impl Repo,
    access: Access,
    request: &GitRequest<'_>,
    options: &ServeOptions,
) -> Served {
    let plain = |response| Served {
        response,
        pushed: Vec::new(),
    };
    let service = if request.rest == "info/refs" {
        query_service(request.query)
    } else {
        Some(request.rest)
    };
    let service = match service {
        Some(s @ ("git-upload-pack" | "git-receive-pack")) => s,
        _ => {
            return plain(text(
                403,
                "only smart HTTP (?service=git-upload-pack or git-receive-pack) is served\n",
            ));
        }
    };
    if service == "git-receive-pack" && access != Access::Write {
        return plain(text(403, "this token cannot push to this repository\n"));
    }
    if request.rest == "info/refs" {
        if request.method != "GET" && request.method != "HEAD" {
            return plain(text(405, "use GET\n"));
        }
        let adv = if service == "git-upload-pack" {
            upload_advertisement(repo).await
        } else {
            receive_advertisement(repo).await
        };
        return match adv {
            Ok(body) => {
                let body = if request.method == "HEAD" {
                    Vec::new()
                } else {
                    body
                };
                plain(binary(
                    &format!("application/x-{service}-advertisement"),
                    body,
                ))
            }
            Err(e) => plain(text(500, &format!("{e}\n"))),
        };
    }
    if request.method != "POST" {
        return plain(text(405, "use POST\n"));
    }
    let limit = options.body_limit(request.rest);
    if request.body.len() > limit {
        return plain(too_large(limit));
    }
    let encoding = request.content_encoding.unwrap_or("").to_ascii_lowercase();
    let inflated;
    let body = if encoding == "gzip" || encoding == "x-gzip" {
        let cap = if service == "git-receive-pack" {
            options.max_push_bytes
        } else {
            options.max_inflated_bytes
        };
        match gunzip(request.body, cap) {
            Ok(b) => {
                inflated = b;
                &inflated[..]
            }
            Err("too large") => {
                return plain(text(
                    413,
                    &format!("the request body is over {cap} bytes inflated\n"),
                ));
            }
            Err(e) => return plain(text(400, &format!("{e}\n"))),
        }
    } else {
        request.body
    };
    if service == "git-upload-pack" {
        plain(binary(
            "application/x-git-upload-pack-result",
            upload_pack(repo, body).await,
        ))
    } else {
        let outcome = receive_pack(repo, body, &options.receive).await;
        Served {
            response: binary("application/x-git-receive-pack-result", outcome.body),
            pushed: outcome.updated,
        }
    }
}

// ── Advertisements ──────────────────────────────────────────────────────────

/// The `# service=` preamble smart HTTP puts before an advertisement.
fn preamble(service: &str) -> Vec<u8> {
    let mut out = pktline::line(format!("# service={service}\n"));
    out.extend_from_slice(FLUSH);
    out
}

fn ref_lines(out: &mut Vec<u8>, lines: &[(String, ObjectId)], caps: &str) {
    if lines.is_empty() {
        pktline::write(
            out,
            format!("{} capabilities^{{}}\0{caps}\n", ObjectId::ZERO).as_bytes(),
        );
    }
    for (i, (name, id)) in lines.iter().enumerate() {
        let line = if i == 0 {
            format!("{id} {name}\0{caps}\n")
        } else {
            format!("{id} {name}\n")
        };
        pktline::write(out, line.as_bytes());
    }
    out.extend_from_slice(FLUSH);
}

/// The upload-pack advertisement: the service line, `HEAD` (when it
/// resolves) with the capabilities, then every ref, each annotated tag
/// followed by its peeled `^{}` line.
pub async fn upload_advertisement(repo: &impl Repo) -> Result<Vec<u8>> {
    let refs = repo.refs().await?;
    let head = repo.head_ref().to_string();
    let mut caps = vec![
        "multi_ack_detailed",
        "side-band",
        "side-band-64k",
        "no-progress",
        "object-format=sha1",
        AGENT,
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    let mut lines = Vec::new();
    if let Some(id) = refs.get(&head) {
        caps.insert(0, format!("symref=HEAD:{head}"));
        lines.push(("HEAD".to_string(), *id));
    }
    for (name, id) in &refs {
        lines.push((name.clone(), *id));
        if name.starts_with("refs/tags/")
            && let Some(o) = repo.read_object(id).await?
            && o.kind == ObjectKind::Tag
            && let Some(peeled) = peel(repo, o).await?
        {
            lines.push((format!("{name}^{{}}"), peeled));
        }
    }
    let mut out = preamble("git-upload-pack");
    ref_lines(&mut out, &lines, &caps.join(" "));
    Ok(out)
}

/// The receive-pack advertisement: the service line and every ref, the
/// first carrying the capabilities.
pub async fn receive_advertisement(repo: &impl Repo) -> Result<Vec<u8>> {
    let refs = repo.refs().await?;
    let lines: Vec<(String, ObjectId)> = refs.into_iter().collect();
    let caps = [
        "report-status",
        "delete-refs",
        "atomic",
        "ofs-delta",
        "object-format=sha1",
        AGENT,
    ]
    .join(" ");
    let mut out = preamble("git-receive-pack");
    ref_lines(&mut out, &lines, &caps);
    Ok(out)
}

/// The non-tag object a tag object (chain) names.
async fn peel(repo: &impl Repo, mut o: Object) -> Result<Option<ObjectId>> {
    for _ in 0..16 {
        let Some((target, kind)) = tag_target(&o.data) else {
            return Ok(None);
        };
        if kind != ObjectKind::Tag {
            return Ok(Some(target));
        }
        match repo.read_object(&target).await? {
            Some(next) => o = next,
            None => return Ok(None),
        }
    }
    Ok(None)
}

// ── Upload-pack ─────────────────────────────────────────────────────────────

/// One line of an upload-pack request, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadLine {
    Want(ObjectId),
    Have(ObjectId),
    Flush,
    Done,
}

/// An upload-pack request: its lines, the first want's capabilities, and
/// what it wants.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UploadRequest {
    pub lines: Vec<UploadLine>,
    pub wants: Vec<ObjectId>,
    pub caps: HashSet<String>,
}

/// Parses an upload-pack request body; the error names what is malformed,
/// unsupported or too many.
pub fn parse_upload_request(body: &[u8]) -> std::result::Result<UploadRequest, String> {
    let mut req = UploadRequest::default();
    let mut counted = 0;
    for pkt in pktline::read_all(body).map_err(|e| e.to_string())? {
        let Some(line) = pkt.text() else {
            if pkt == Packet::Flush {
                req.lines.push(UploadLine::Flush);
            }
            continue;
        };
        let mut words = line.split(' ');
        let word = words.next().unwrap_or("");
        let id = words.next().and_then(ObjectId::from_hex);
        match (word, id) {
            ("want" | "have", Some(id)) => {
                counted += 1;
                if counted > MAX_LINES {
                    return Err(format!("more than {MAX_LINES} want and have lines"));
                }
                if word == "want" {
                    if req.wants.is_empty() {
                        req.caps.extend(words.map(String::from));
                    }
                    req.wants.push(id);
                    req.lines.push(UploadLine::Want(id));
                } else {
                    req.lines.push(UploadLine::Have(id));
                }
            }
            _ if line == "done" => req.lines.push(UploadLine::Done),
            ("shallow" | "deepen" | "deepen-since" | "deepen-not" | "filter", _) => {
                return Err(format!("{word} is not supported by this server"));
            }
            _ => {
                let shown: String = line.chars().take(80).collect();
                return Err(format!("unexpected line {shown:?}"));
            }
        }
    }
    Ok(req)
}

fn err_packet(message: &str) -> Vec<u8> {
    pktline::line(format!("ERR {message}\n"))
}

async fn read_as(repo: &impl Repo, id: &ObjectId, kind: ObjectKind) -> Result<Vec<u8>> {
    match repo.read_object(id).await? {
        Some(o) if o.kind == kind => Ok(o.data),
        _ => Err(GitError::unavailable(format!("{kind} {id} is missing"))),
    }
}

async fn is_commit(repo: &impl Repo, id: &ObjectId) -> Result<bool> {
    Ok(matches!(repo.read_object(id).await?, Some(o) if o.kind == ObjectKind::Commit))
}

/// The commit a want names, through annotated tags; `None` for a want of a
/// tree or blob.
async fn want_commit(repo: &impl Repo, id: &ObjectId) -> Result<Option<ObjectId>> {
    let Some(o) = repo.read_object(id).await? else {
        return Err(GitError::unavailable(format!("object {id} is missing")));
    };
    match o.kind {
        ObjectKind::Commit => Ok(Some(*id)),
        ObjectKind::Tag => {
            let Some(target) = peel(repo, o).await? else {
                return Ok(None);
            };
            Ok(is_commit(repo, &target).await?.then_some(target))
        }
        _ => Ok(None),
    }
}

/// Every commit the commits and annotated tags in `tips` reach.
async fn commits_reached(repo: &impl Repo, tips: &HashSet<ObjectId>) -> Result<HashSet<ObjectId>> {
    let mut seen = HashSet::new();
    let mut stack = Vec::new();
    for t in tips {
        if let Some(c) = want_commit(repo, t).await? {
            stack.push(c);
        }
    }
    while let Some(h) = stack.pop() {
        if !seen.insert(h) {
            continue;
        }
        stack.extend(Commit::parse(&read_as(repo, &h, ObjectKind::Commit).await?)?.parents);
    }
    Ok(seen)
}

/// Whether every want descends from (or is) a commit in `common`.
async fn every_want_reached(
    repo: &impl Repo,
    wants: &[ObjectId],
    common: &HashSet<ObjectId>,
) -> Result<bool> {
    if common.is_empty() {
        return Ok(false);
    }
    let unique: HashSet<ObjectId> = wants.iter().copied().collect();
    for want in unique {
        let Some(start) = want_commit(repo, &want).await? else {
            return Ok(false);
        };
        let mut seen = HashSet::new();
        let mut stack = vec![start];
        let mut reached = false;
        while let Some(h) = stack.pop() {
            if !seen.insert(h) {
                continue;
            }
            if common.contains(&h) {
                reached = true;
                break;
            }
            stack.extend(Commit::parse(&read_as(repo, &h, ObjectKind::Commit).await?)?.parents);
        }
        if !reached {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The upload-pack answer to `body`: the negotiation's ACK and NAK lines,
/// and after `done` the pack. A request that cannot be served (malformed,
/// unsupported, a want no ref reaches, a missing object) answers one
/// `ERR` packet.
pub async fn upload_pack(repo: &impl Repo, body: &[u8]) -> Vec<u8> {
    let req = match parse_upload_request(body) {
        Ok(r) => r,
        Err(e) => return err_packet(&e),
    };
    if req.wants.is_empty() {
        return Vec::new();
    }
    match negotiate(repo, &req).await {
        Ok(out) => out,
        Err(e) => err_packet(&format!("upload-pack: {e}")),
    }
}

async fn negotiate(repo: &impl Repo, req: &UploadRequest) -> Result<Vec<u8>> {
    let refs = repo.refs().await?;
    let tips: HashSet<ObjectId> = refs.values().copied().collect();
    // As git's upload-pack in stateless mode: a want that is no tip is
    // served when a tip reaches it, since a ref may move between the
    // advertisement and a later round.
    let mut reachable: Option<HashSet<ObjectId>> = None;
    for w in &req.wants {
        if tips.contains(w) {
            continue;
        }
        if reachable.is_none() {
            reachable = Some(commits_reached(repo, &tips).await?);
        }
        if !reachable.as_ref().is_some_and(|r| r.contains(w)) {
            return Ok(err_packet(&format!("upload-pack: not our ref {w}")));
        }
    }
    let detailed = req.caps.contains("multi_ack_detailed");
    let mut out = Vec::new();
    let mut common = HashSet::new();
    let mut last: Option<ObjectId> = None;
    let mut got_common = false;
    let mut got_other = false;
    // Memoized on the common set's size: it only grows.
    let mut ready_at = usize::MAX;
    let mut ready = false;
    let mut done = false;
    // The first flush ends the wants; the next one ends a negotiation round.
    let mut in_haves = false;
    for line in &req.lines {
        match line {
            UploadLine::Want(_) => {}
            UploadLine::Have(id) => {
                if is_commit(repo, id).await? {
                    common.insert(*id);
                    last = Some(*id);
                    got_common = true;
                    if detailed {
                        pktline::write(&mut out, format!("ACK {id} common\n").as_bytes());
                    }
                } else {
                    got_other = true;
                    if detailed {
                        if ready_at != common.len() {
                            ready = every_want_reached(repo, &req.wants, &common).await?;
                            ready_at = common.len();
                        }
                        if ready {
                            pktline::write(&mut out, format!("ACK {id} ready\n").as_bytes());
                        }
                    }
                }
            }
            UploadLine::Flush => {
                if !in_haves {
                    in_haves = true;
                    continue;
                }
                if detailed
                    && got_common
                    && !got_other
                    && let Some(l) = last
                {
                    let ready = if ready_at == common.len() {
                        ready
                    } else {
                        every_want_reached(repo, &req.wants, &common).await?
                    };
                    if ready {
                        pktline::write(&mut out, format!("ACK {l} ready\n").as_bytes());
                    }
                }
                pktline::write(&mut out, b"NAK\n");
                // Stateless: a round ends the request.
                return Ok(out);
            }
            UploadLine::Done => {
                done = true;
                break;
            }
        }
    }
    if !done {
        return Ok(out);
    }
    match (detailed, last) {
        (true, Some(l)) => pktline::write(&mut out, format!("ACK {l}\n").as_bytes()),
        _ => pktline::write(&mut out, b"NAK\n"),
    }
    let common: Vec<ObjectId> = common.into_iter().collect();
    let objects = pack_objects(repo, &req.wants, &common).await?;
    let pack = write_pack(&objects);
    let band = if req.caps.contains("side-band-64k") {
        SIDE_BAND_64K_PAYLOAD
    } else if req.caps.contains("side-band") {
        SIDE_BAND_PAYLOAD
    } else {
        0
    };
    if band == 0 {
        out.extend_from_slice(&pack);
        return Ok(out);
    }
    pktline::write_sideband(&mut out, 1, &pack, band);
    out.extend_from_slice(FLUSH);
    Ok(out)
}

/// The IDs of `tree` and of everything under it, added to `into`.
async fn tree_ids(repo: &impl Repo, tree: ObjectId, into: &mut HashSet<ObjectId>) -> Result<()> {
    let mut stack = vec![tree];
    while let Some(t) = stack.pop() {
        if !into.insert(t) {
            continue;
        }
        for e in parse_tree(&read_as(repo, &t, ObjectKind::Tree).await?)? {
            if e.mode == Mode::GITLINK {
                continue;
            }
            if e.mode.is_tree() {
                stack.push(e.id);
            } else {
                into.insert(e.id);
            }
        }
    }
    Ok(())
}

/// `tree` and everything under it not in `skip`, added to `into`.
async fn add_tree(
    repo: &impl Repo,
    tree: ObjectId,
    skip: &HashSet<ObjectId>,
    into: &mut Vec<Object>,
    have: &mut HashSet<ObjectId>,
) -> Result<()> {
    let mut stack = vec![tree];
    while let Some(t) = stack.pop() {
        if skip.contains(&t) || !have.insert(t) {
            continue;
        }
        let data = read_as(repo, &t, ObjectKind::Tree).await?;
        let entries = parse_tree(&data)?;
        into.push(Object::new(ObjectKind::Tree, data));
        for e in entries {
            if e.mode == Mode::GITLINK {
                continue;
            }
            if e.mode.is_tree() {
                stack.push(e.id);
            } else if !skip.contains(&e.id) && have.insert(e.id) {
                into.push(Object::new(
                    ObjectKind::Blob,
                    read_as(repo, &e.id, ObjectKind::Blob).await?,
                ));
            }
        }
    }
    Ok(())
}

/// The objects reachable from `wants`, less what the client holds for
/// sure: every commit reachable from a common commit, and the trees and
/// files of the common commits themselves. `common` must hold only commits
/// of the repository.
pub async fn pack_objects(
    repo: &impl Repo,
    wants: &[ObjectId],
    common: &[ObjectId],
) -> Result<Vec<Object>> {
    let mut skip = HashSet::new();
    let mut stack = Vec::new();
    for h in common {
        let c = Commit::parse(&read_as(repo, h, ObjectKind::Commit).await?)?;
        tree_ids(repo, c.tree, &mut skip).await?;
        stack.push(*h);
    }
    while let Some(h) = stack.pop() {
        if !skip.insert(h) {
            continue;
        }
        stack.extend(Commit::parse(&read_as(repo, &h, ObjectKind::Commit).await?)?.parents);
    }
    let mut out = Vec::new();
    let mut have = HashSet::new();
    let mut todo: Vec<ObjectId> = wants.to_vec();
    while let Some(h) = todo.pop() {
        if skip.contains(&h) || have.contains(&h) {
            continue;
        }
        let Some(o) = repo.read_object(&h).await? else {
            return Err(GitError::unavailable(format!("object {h} is missing")));
        };
        have.insert(h);
        match o.kind {
            ObjectKind::Commit => {
                let c = Commit::parse(&o.data)?;
                out.push(o);
                add_tree(repo, c.tree, &skip, &mut out, &mut have).await?;
                todo.extend(c.parents);
            }
            ObjectKind::Tag => {
                if let Some((target, _)) = tag_target(&o.data) {
                    todo.push(target);
                }
                out.push(o);
            }
            ObjectKind::Tree => {
                have.remove(&h);
                add_tree(repo, h, &skip, &mut out, &mut have).await?;
            }
            ObjectKind::Blob => out.push(o),
        }
    }
    Ok(out)
}

// ── Receive-pack ────────────────────────────────────────────────────────────

/// What [`receive_pack`] answers: the response body (`report-status`) and
/// the updates it applied.
#[derive(Clone, Debug, Default)]
pub struct ReceiveOutcome {
    pub body: Vec<u8>,
    pub updated: Vec<RefUpdate>,
}

/// Whether `name` is a ref name receive-pack accepts: under `refs/`, as
/// `git check-ref-format` allows.
pub fn valid_ref_name(name: &str) -> bool {
    if !name.starts_with("refs/")
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("@{")
    {
        return false;
    }
    if name
        .bytes()
        .any(|b| b < 0x20 || b == 0x7f || b" ~^:?*[\\".contains(&b))
    {
        return false;
    }
    name.split('/')
        .all(|c| !c.is_empty() && !c.starts_with('.') && !c.ends_with(".lock"))
}

fn report(unpack: &str, updates: &[RefUpdate], statuses: &[RefStatus]) -> Vec<u8> {
    let mut out = Vec::new();
    pktline::write(&mut out, format!("unpack {unpack}\n").as_bytes());
    for (u, s) in updates.iter().zip(statuses) {
        let line = match s {
            Ok(()) => format!("ok {}\n", u.name),
            Err(reason) => format!("ng {} {reason}\n", u.name),
        };
        pktline::write(&mut out, line.as_bytes());
    }
    out.extend_from_slice(FLUSH);
    out
}

/// The IDs an object names, each with the type it must have when known: a
/// commit's tree and parents, a tree's entries (submodules aside), a tag's
/// target.
fn links(o: &Object) -> Result<Vec<(ObjectId, Option<ObjectKind>)>> {
    Ok(match o.kind {
        ObjectKind::Commit => {
            let c = Commit::parse(&o.data)?;
            let mut v = vec![(c.tree, Some(ObjectKind::Tree))];
            v.extend(c.parents.into_iter().map(|p| (p, Some(ObjectKind::Commit))));
            v
        }
        ObjectKind::Tree => parse_tree(&o.data)?
            .into_iter()
            .filter(|e| e.mode != Mode::GITLINK)
            .map(|e| {
                let kind = if e.mode.is_tree() {
                    Some(ObjectKind::Tree)
                } else if e.mode.is_blob() {
                    Some(ObjectKind::Blob)
                } else {
                    None
                };
                (e.id, kind)
            })
            .collect(),
        ObjectKind::Tag => tag_target(&o.data)
            .map(|(id, kind)| vec![(id, Some(kind))])
            .ok_or_else(|| GitError::invalid("A malformed tag object"))?,
        ObjectKind::Blob => Vec::new(),
    })
}

/// Why `tip` cannot be a ref's value, or `None` when every object it
/// reaches is in `pushed` or already in the repository (whose objects are
/// closed under reachability) and has the type that names it: a commit's
/// tree a tree, its parents commits, a tree's entries what their modes say,
/// a tag's target what the tag says.
async fn unconnected(
    repo: &impl Repo,
    pushed: &HashMap<ObjectId, Object>,
    tip: ObjectId,
) -> Result<Option<String>> {
    let mut seen = HashSet::new();
    let mut stack = vec![(tip, None)];
    while let Some((id, want)) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let missing = || Ok(Some("missing necessary objects".to_string()));
        let kind = match (pushed.get(&id), want) {
            (Some(o), _) => {
                stack.extend(links(o)?);
                o.kind
            }
            (None, None) => {
                if !repo.has_object(&id).await? {
                    return missing();
                }
                continue;
            }
            (None, Some(_)) => match repo.object_kind(&id).await? {
                Some(k) => k,
                None => return missing(),
            },
        };
        if let Some(want) = want
            && want != kind
        {
            return Ok(Some(format!("{id} is a {kind} where a {want} belongs")));
        }
    }
    Ok(None)
}

async fn read_commit(
    repo: &impl Repo,
    pushed: &HashMap<ObjectId, Object>,
    id: &ObjectId,
) -> Result<Option<Commit>> {
    let o = match pushed.get(id) {
        Some(o) => Some(o.clone()),
        None => repo.read_object(id).await?,
    };
    match o {
        Some(o) if o.kind == ObjectKind::Commit => Ok(Some(Commit::parse(&o.data)?)),
        _ => Ok(None),
    }
}

/// Whether commit `new` descends from (or is) `old`.
async fn descends(
    repo: &impl Repo,
    pushed: &HashMap<ObjectId, Object>,
    new: ObjectId,
    old: ObjectId,
) -> Result<bool> {
    let mut seen = HashSet::new();
    let mut stack = vec![new];
    while let Some(id) = stack.pop() {
        if id == old {
            return Ok(true);
        }
        if !seen.insert(id) {
            continue;
        }
        if let Some(c) = read_commit(repo, pushed, &id).await? {
            stack.extend(c.parents);
        }
    }
    Ok(false)
}

/// Answers a receive-pack request body: parses the commands and the pack,
/// checks each update (see the module documentation), applies those that
/// pass and reports on each.
pub async fn receive_pack(
    repo: &impl Repo,
    body: &[u8],
    options: &ReceiveOptions,
) -> ReceiveOutcome {
    match receive(repo, body, options).await {
        Ok(o) => o,
        Err(e) => ReceiveOutcome {
            body: err_packet(&e.to_string()),
            updated: Vec::new(),
        },
    }
}

async fn receive(
    repo: &impl Repo,
    body: &[u8],
    options: &ReceiveOptions,
) -> Result<ReceiveOutcome> {
    let (packets, end) = pktline::read(body, true)?;
    let mut updates = Vec::new();
    let mut caps = HashSet::new();
    for pkt in &packets {
        let Some(line) = pkt.text() else { continue };
        let (cmd, cap_text) = match line.split_once('\0') {
            Some((c, k)) => (c.to_string(), Some(k.to_string())),
            None => (line.clone(), None),
        };
        if updates.is_empty()
            && let Some(k) = cap_text
        {
            caps.extend(k.split(' ').filter(|c| !c.is_empty()).map(String::from));
        }
        let mut parts = cmd.splitn(3, ' ');
        let (Some(old), Some(new), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(GitError::invalid(format!(
                "protocol error: expected old/new/ref, got {cmd:?}"
            )));
        };
        let (Some(old), Some(new)) = (ObjectId::from_hex(old), ObjectId::from_hex(new)) else {
            return Err(GitError::invalid(format!(
                "protocol error: expected old/new/ref, got {cmd:?}"
            )));
        };
        updates.push(RefUpdate {
            name: name.to_string(),
            old,
            new,
        });
    }
    if updates.is_empty() {
        return Ok(ReceiveOutcome::default());
    }
    let atomic = caps.contains("atomic");
    let pack_bytes = &body[end..];

    // Unpack.
    let mut pushed: HashMap<ObjectId, Object> = HashMap::new();
    if !pack_bytes.is_empty() {
        match unpack(repo, pack_bytes, options).await {
            Ok(objects) => {
                for o in objects {
                    pushed.insert(o.id(), o);
                }
            }
            Err(e) => {
                let why = e.to_string().replace('\n', " ");
                let statuses: Vec<RefStatus> = updates
                    .iter()
                    .map(|_| Err("unpacker error".to_string()))
                    .collect();
                return Ok(ReceiveOutcome {
                    body: report(&why, &updates, &statuses),
                    updated: Vec::new(),
                });
            }
        }
    }

    // Check each update.
    let current = repo.refs().await?;
    let head = repo.head_ref().to_string();
    let mut statuses: Vec<RefStatus> = Vec::with_capacity(updates.len());
    let mut names = HashSet::new();
    for u in &updates {
        statuses.push(check_update(repo, &pushed, &current, &head, u, options, &mut names).await?);
    }
    if atomic && statuses.iter().any(|s| s.is_err()) {
        for s in &mut statuses {
            if s.is_ok() {
                *s = Err(ATOMIC_FAILED.to_string());
            }
        }
        return Ok(ReceiveOutcome {
            body: report("ok", &updates, &statuses),
            updated: Vec::new(),
        });
    }

    // Apply what passed.
    let passing: Vec<RefUpdate> = updates
        .iter()
        .zip(&statuses)
        .filter(|(_, s)| s.is_ok())
        .map(|(u, _)| u.clone())
        .collect();
    let mut updated = Vec::new();
    if !passing.is_empty() {
        let applied = repo
            .apply(pushed.into_values().collect(), &passing, atomic)
            .await?;
        let mut applied = passing.iter().zip(applied);
        for s in statuses.iter_mut().filter(|s| s.is_ok()) {
            let (u, a) = applied.next().expect("one status per passing update");
            if a.is_ok() {
                updated.push(u.clone());
            }
            *s = a;
        }
    }
    Ok(ReceiveOutcome {
        body: report("ok", &updates, &statuses),
        updated,
    })
}

async fn unpack(repo: &impl Repo, pack: &[u8], options: &ReceiveOptions) -> Result<Vec<Object>> {
    let max_object_bytes = options.max_object_bytes;
    let parsed = ParsedPack::parse_bounded(pack, max_object_bytes, options.max_unpacked_bytes)?;
    let mut external = HashMap::new();
    loop {
        match parsed.resolve(&external, max_object_bytes)? {
            Resolved::Objects(objects) => {
                for o in &objects {
                    // Each object must parse as its type.
                    links(o)?;
                }
                return Ok(objects);
            }
            Resolved::Missing(ids) => {
                // A missing base may be the result of another delta of this
                // pack whose own base is in the repository: fetch what the
                // repository holds and resolve again.
                let mut fetched = false;
                let mut lacking = None;
                for id in ids {
                    if external.contains_key(&id) {
                        continue;
                    }
                    match repo.read_object(&id).await? {
                        Some(o) => {
                            external.insert(id, o);
                            fetched = true;
                        }
                        None => lacking = lacking.or(Some(id)),
                    }
                }
                if !fetched {
                    return Err(match lacking {
                        Some(id) => GitError::invalid(format!("missing delta base {id}")),
                        None => GitError::invalid("a delta chain does not resolve"),
                    });
                }
            }
        }
    }
}

async fn check_update(
    repo: &impl Repo,
    pushed: &HashMap<ObjectId, Object>,
    current: &BTreeMap<String, ObjectId>,
    head: &str,
    u: &RefUpdate,
    options: &ReceiveOptions,
    names: &mut HashSet<String>,
) -> Result<RefStatus> {
    if !valid_ref_name(&u.name) {
        return Ok(Err("funny refname".into()));
    }
    if !names.insert(u.name.clone()) {
        return Ok(Err("duplicate ref update".into()));
    }
    if options.protect_version_tags && is_version_tag(&u.name) {
        return Ok(Err(VERSION_TAG_REFUSED.into()));
    }
    let at = current.get(&u.name).copied().unwrap_or(ObjectId::ZERO);
    if at != u.old {
        return Ok(Err("stale info".into()));
    }
    if u.is_delete() {
        if u.old.is_zero() {
            return Ok(Err("nothing to delete".into()));
        }
        if u.name == head {
            return Ok(Err("deletion of the current branch prohibited".into()));
        }
        if options.deny_deletes {
            return Ok(Err("deletion prohibited".into()));
        }
        return Ok(Ok(()));
    }
    if let Some(why) = unconnected(repo, pushed, u.new).await? {
        return Ok(Err(why));
    }
    if u.name.starts_with("refs/heads/") {
        if read_commit(repo, pushed, &u.new).await?.is_none() {
            return Ok(Err("a branch must name a commit".into()));
        }
        if options.deny_non_fast_forwards
            && !u.old.is_zero()
            && !descends(repo, pushed, u.new, u.old).await?
        {
            return Ok(Err("non-fast-forward".into()));
        }
    }
    Ok(Ok(()))
}
