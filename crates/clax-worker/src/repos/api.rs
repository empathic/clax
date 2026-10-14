//! `RepoDO`'s internal API, which only Durable Object stubs reach (the
//! Worker forwards `/git/` to it and nothing else): the stand-in's
//! repositories ([`DoHost`], a [`RepoHost`]) and repository tokens
//! ([`DoTokens`]), each call one HTTP request to `http://repos/_repos/…`.
//!
//! - `GET open?repo=` → 200, or 404 when missing; `POST create?repo=`,
//!   `POST fork?from=&to=`, `POST delete?repo=`, `POST prune?repo=` (drops
//!   the objects no ref of the repository reaches).
//! - `GET refs?repo=` → `{"<ref>": "<hex>"}`.
//! - `GET object?repo=&id=` → 200 with the type's pack code then the
//!   content, or 204 when the repository does not hold it.
//! - `POST apply?repo=&atomic=0|1`, body: a 4-byte big-endian length, that
//!   many bytes of JSON `[[ref, old, new], …]`, then each object as its
//!   pack code, a 4-byte big-endian length and its content → JSON
//!   `[null | "<reason>", …]`.
//! - `POST git/authorize` `{"path", "authorization"}` → `{"access":
//!   "read" | "write" | null}`: what a `/git/` request's `Authorization`
//!   header grants on the repository its path names.
//! - `POST token/mint` `{"repo", "scope", "ttl_s", "origin", "person",
//!   "session", "link"}` (the last three the token's [`tokens::Holder`]) → a
//!   [`GitToken`]; `POST token/revoke` `{"repo", "id"}` → `{"revoked"}`.
//! - `GET token/held` → `[`[`tokens::HeldToken`]`, …]`: the live tokens
//!   handed to a person, an agent session or a page's link; `POST
//!   token/revoke-held`
//!   `{"person", "session"}` (either may be absent) → `{"revoked": n}`.
//! - `POST token/live` `{"repo"}` → `{"tokens": [{"id", "scope", "person",
//!   "session"}, …]}`: the repository's live tokens and their holders.
//! - `POST push/pushers` `{"repo", "commits": ["<hex>", …]}` → `{"pushers":
//!   [null | {"person", "session"}, …]}`: who made the newest push leaving
//!   `main` at each commit ([`DoLedger`] serves clax-core's push ledger
//!   from these).
//!
//! An error is a status (400 `invalid_argument`, 404 `not_found`, 409
//! `version_conflict`, 503 `repo_unavailable`) with `{"code", "message"}`.

use std::collections::BTreeMap;
use std::sync::Arc;

use clax_core::working::Clock;
use clax_git::http::{HttpRequest, HttpResponse, Transport};
use clax_git::{GitError, Object, ObjectId, ObjectKind, RefStatus, RefUpdate, Repo, RepoHost};
use serde_json::{Value, json};

use super::db;
use super::local::{AsConn, LocalHost, malformed};
use super::tokens::{self, GitToken, Holder, Scope, TokenHost};

/// The base of every internal URL.
pub const BASE: &str = "http://repos/_repos/";
/// [`BASE`]'s host. `RepoDO` answers its internal API only on it: a
/// request the Worker forwards keeps the public host it came to.
pub const INTERNAL_HOST: &str = "repos";

// ── Encoding ────────────────────────────────────────────────────────────────

fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn dec(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| dec(v)).flatten()
    })
}

fn url(op: &str, params: &[(&str, &str)]) -> String {
    let q: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", enc(v)))
        .collect();
    if q.is_empty() {
        format!("{BASE}{op}")
    } else {
        format!("{BASE}{op}?{}", q.join("&"))
    }
}

/// The body of `POST apply`.
pub fn encode_apply(objects: &[Object], updates: &[RefUpdate]) -> Vec<u8> {
    let ups: Vec<Value> = updates
        .iter()
        .map(|u| json!([u.name, u.old.to_hex(), u.new.to_hex()]))
        .collect();
    let head = serde_json::to_vec(&ups).expect("JSON of strings");
    let mut out = Vec::with_capacity(
        4 + head.len() + objects.iter().map(|o| 5 + o.data.len()).sum::<usize>(),
    );
    out.extend_from_slice(&(head.len() as u32).to_be_bytes());
    out.extend_from_slice(&head);
    for o in objects {
        out.push(o.kind.pack_code());
        out.extend_from_slice(&(o.data.len() as u32).to_be_bytes());
        out.extend_from_slice(&o.data);
    }
    out
}

fn take<'a>(b: &mut &'a [u8], n: usize) -> clax_git::Result<&'a [u8]> {
    if b.len() < n {
        return Err(malformed("apply body truncated"));
    }
    let (head, rest) = b.split_at(n);
    *b = rest;
    Ok(head)
}

fn be32(b: &[u8]) -> usize {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
}

fn hex_id(s: &str) -> clax_git::Result<ObjectId> {
    ObjectId::from_hex(s).ok_or_else(|| malformed("object ID"))
}

/// Parses the body of `POST apply`.
pub fn decode_apply(mut body: &[u8]) -> clax_git::Result<(Vec<Object>, Vec<RefUpdate>)> {
    let n = be32(take(&mut body, 4)?);
    let ups: Vec<(String, String, String)> =
        serde_json::from_slice(take(&mut body, n)?).map_err(|_| malformed("apply updates"))?;
    let updates = ups
        .into_iter()
        .map(|(name, old, new)| {
            Ok(RefUpdate {
                name,
                old: hex_id(&old)?,
                new: hex_id(&new)?,
            })
        })
        .collect::<clax_git::Result<_>>()?;
    let mut objects = Vec::new();
    while !body.is_empty() {
        let kind = ObjectKind::from_pack_code(take(&mut body, 1)?[0])
            .ok_or_else(|| malformed("object type"))?;
        let n = be32(take(&mut body, 4)?);
        objects.push(Object::new(kind, take(&mut body, n)?.to_vec()));
    }
    Ok((objects, updates))
}

fn error_response(e: &GitError) -> HttpResponse {
    let status = match e {
        GitError::InvalidArgument(_) => 400,
        GitError::NotFound(_) => 404,
        GitError::VersionConflict { .. } => 409,
        GitError::Auth { .. } | GitError::RepoUnavailable(_) => 503,
    };
    json_response(
        status,
        &json!({ "code": e.code(), "message": e.to_string() }),
    )
}

fn json_response(status: u16, v: &Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![("content-type".into(), "application/json".into())],
        body: serde_json::to_vec(v).expect("JSON"),
    }
}

/// The error an error response carries.
fn error_of(r: &HttpResponse) -> GitError {
    let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
    let message = v["message"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| format!("The repository store answered {}", r.status));
    match v["code"].as_str() {
        Some("invalid_argument") => GitError::InvalidArgument(message),
        Some("not_found") => GitError::NotFound(message),
        Some("version_conflict") => GitError::VersionConflict {
            message,
            head: None,
        },
        _ => GitError::RepoUnavailable(message),
    }
}

// ── Server ──────────────────────────────────────────────────────────────────

/// One internal request: the path after [`BASE`], the query and the body.
pub struct InternalRequest<'a> {
    pub method: &'a str,
    pub op: &'a str,
    pub query: Option<&'a str>,
    pub body: &'a [u8],
}

/// What `RepoDO` serves its internal API from: the stand-in's database
/// (repositories and ledger) and the host its tokens come from.
pub struct RepoService<C, T> {
    pub host: LocalHost<C>,
    pub tokens: T,
    pub clock: Arc<dyn Clock>,
}

impl<C: AsConn + clax_git::MaybeSend, T: TokenHost> RepoService<C, T> {
    pub fn now_ms(&self) -> i64 {
        self.clock.now().timestamp_millis()
    }

    /// Answers one internal request.
    pub async fn serve(&self, req: &InternalRequest<'_>) -> HttpResponse {
        match self.answer(req).await {
            Ok(r) => r,
            Err(e) => error_response(&e),
        }
    }

    async fn answer(&self, req: &InternalRequest<'_>) -> clax_git::Result<HttpResponse> {
        let q = |k: &str| query_param(req.query, k).ok_or_else(|| malformed(k));
        let ok = || json_response(200, &json!({}));
        let h = &self.host;
        match (req.method, req.op) {
            ("GET", "open") => {
                let repo = q("repo")?;
                if h.with(|c| db::exists(c, &repo))? {
                    Ok(ok())
                } else {
                    Err(GitError::NotFound(format!("No repository {repo}")))
                }
            }
            ("POST", "create") => {
                h.with(|c| db::create(c, &q("repo")?))?;
                Ok(ok())
            }
            ("POST", "fork") => {
                h.with(|c| db::fork(c, &q("from")?, &q("to")?))?;
                Ok(ok())
            }
            ("POST", "prune") => {
                h.with(|c| db::prune(c, &q("repo")?))?;
                Ok(ok())
            }
            ("POST", "delete") => {
                let now = self.now_ms();
                h.with(|c| db::delete(c, &q("repo")?, now))?;
                Ok(ok())
            }
            ("GET", "refs") => {
                let refs = h.with(|c| db::refs(c, &q("repo")?))?;
                let m: BTreeMap<String, String> =
                    refs.into_iter().map(|(k, v)| (k, v.to_hex())).collect();
                Ok(json_response(200, &json!(m)))
            }
            ("GET", "object") => {
                let id = hex_id(&q("id")?)?;
                match h.with(|c| db::read_object(c, &q("repo")?, &id))? {
                    None => Ok(HttpResponse::new(204, Vec::new())),
                    Some(o) => {
                        let mut body = Vec::with_capacity(1 + o.data.len());
                        body.push(o.kind.pack_code());
                        body.extend_from_slice(&o.data);
                        Ok(HttpResponse::new(200, body))
                    }
                }
            }
            ("POST", "apply") => {
                let atomic = q("atomic")? == "1";
                let (objects, updates) = decode_apply(req.body)?;
                let st = h.with(|c| db::apply(c, &q("repo")?, &objects, &updates, atomic))?;
                let v: Vec<Value> = st
                    .into_iter()
                    .map(|s| s.err().map_or(Value::Null, Value::String))
                    .collect();
                Ok(json_response(200, &json!(v)))
            }
            ("POST", "git/authorize") => {
                let v: Value =
                    serde_json::from_slice(req.body).map_err(|_| malformed("authorize request"))?;
                let path = v["path"].as_str().ok_or_else(|| malformed("path"))?;
                let access = super::git_http::authorize(
                    h,
                    path,
                    v["authorization"].as_str(),
                    self.now_ms(),
                )?;
                let access = access.map(|a| match a {
                    clax_git::server::Access::Read => "read",
                    clax_git::server::Access::Write => "write",
                });
                Ok(json_response(200, &json!({ "access": access })))
            }
            ("POST", "token/mint") => {
                let v: Value =
                    serde_json::from_slice(req.body).map_err(|_| malformed("token request"))?;
                let repo = v["repo"].as_str().ok_or_else(|| malformed("repo"))?;
                let scope = v["scope"].as_str().and_then(Scope::parse).ok_or_else(|| {
                    GitError::InvalidArgument("scope must be read or write".into())
                })?;
                let ttl = v["ttl_s"]
                    .as_u64()
                    .and_then(|t| u32::try_from(t).ok())
                    .ok_or_else(|| malformed("ttl_s"))?;
                let holder = Holder {
                    person: v["person"].as_str().map(String::from),
                    session: v["session"].as_str().map(String::from),
                    link: v["link"].as_bool().unwrap_or(false),
                };
                let token = tokens::mint(
                    h,
                    &self.tokens,
                    repo,
                    scope,
                    ttl,
                    v["origin"].as_str(),
                    &holder,
                    self.now_ms(),
                )
                .await?;
                Ok(json_response(200, &json!(token)))
            }
            ("POST", "token/revoke") => {
                let v: Value =
                    serde_json::from_slice(req.body).map_err(|_| malformed("revoke request"))?;
                let repo = v["repo"].as_str().ok_or_else(|| malformed("repo"))?;
                let id = v["id"].as_str().ok_or_else(|| malformed("id"))?;
                let revoked = tokens::revoke(h, &self.tokens, repo, id, self.now_ms()).await?;
                Ok(json_response(200, &json!({ "revoked": revoked })))
            }
            ("GET", "token/held") => {
                let held = tokens::held(h, self.now_ms())?;
                Ok(json_response(200, &json!(held)))
            }
            ("POST", "token/revoke-held") => {
                let v: Value =
                    serde_json::from_slice(req.body).map_err(|_| malformed("revoke request"))?;
                let revoked = tokens::revoke_held(
                    h,
                    &self.tokens,
                    v["person"].as_str(),
                    v["session"].as_str(),
                    self.now_ms(),
                )
                .await?;
                Ok(json_response(200, &json!({ "revoked": revoked })))
            }
            ("POST", "token/live") => {
                let v: Value =
                    serde_json::from_slice(req.body).map_err(|_| malformed("tokens request"))?;
                let repo = v["repo"].as_str().ok_or_else(|| malformed("repo"))?;
                let now = self.now_ms();
                let rows = h.with(|c| db::live_tokens(c, repo, now))?;
                let tokens: Vec<Value> = rows
                    .into_iter()
                    .map(|r| {
                        json!({
                            "id": r.id,
                            "scope": r.scope,
                            "person": r.holder.person,
                            "session": r.holder.session,
                        })
                    })
                    .collect();
                Ok(json_response(200, &json!({ "tokens": tokens })))
            }
            ("POST", "push/pushers") => {
                let v: Value =
                    serde_json::from_slice(req.body).map_err(|_| malformed("pushers request"))?;
                let repo = v["repo"].as_str().ok_or_else(|| malformed("repo"))?;
                let commits = v["commits"]
                    .as_array()
                    .ok_or_else(|| malformed("commits"))?;
                let mut pushers = Vec::with_capacity(commits.len());
                for c in commits {
                    let c = c.as_str().ok_or_else(|| malformed("commits"))?;
                    pushers.push(h.with(|conn| db::pusher(conn, repo, c))?);
                }
                Ok(json_response(200, &json!({ "pushers": pushers })))
            }
            _ => Err(GitError::InvalidArgument(format!(
                "No repository store call {} {}",
                req.method, req.op
            ))),
        }
    }
}

// ── Client ──────────────────────────────────────────────────────────────────

fn request(method: &'static str, url: String, body: Vec<u8>) -> HttpRequest {
    HttpRequest {
        method,
        url,
        headers: Vec::new(),
        body,
    }
}

async fn call(t: &impl Transport, r: HttpRequest) -> clax_git::Result<HttpResponse> {
    let res = t.send(r).await?;
    if (200..300).contains(&res.status) {
        Ok(res)
    } else {
        Err(error_of(&res))
    }
}

/// The stand-in's repositories, reached through `RepoDO`'s internal API
/// over `transport` (a stub's `fetch` in a Worker).
#[derive(Clone)]
pub struct DoHost<T> {
    pub transport: T,
}

/// One repository of a [`DoHost`].
pub struct DoRepo<T> {
    transport: T,
    name: String,
}

impl<T: Transport + Clone> RepoHost for DoHost<T> {
    type Repo = DoRepo<T>;

    async fn open(&self, name: &str) -> clax_git::Result<Option<DoRepo<T>>> {
        match call(
            &self.transport,
            request("GET", url("open", &[("repo", name)]), Vec::new()),
        )
        .await
        {
            Ok(_) => Ok(Some(DoRepo {
                transport: self.transport.clone(),
                name: name.to_string(),
            })),
            Err(GitError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn create(&self, name: &str) -> clax_git::Result<()> {
        call(
            &self.transport,
            request("POST", url("create", &[("repo", name)]), Vec::new()),
        )
        .await
        .map(drop)
    }

    async fn fork(&self, from: &str, to: &str) -> clax_git::Result<()> {
        let u = url("fork", &[("from", from), ("to", to)]);
        call(&self.transport, request("POST", u, Vec::new()))
            .await
            .map(drop)
    }

    async fn delete(&self, name: &str) -> clax_git::Result<()> {
        call(
            &self.transport,
            request("POST", url("delete", &[("repo", name)]), Vec::new()),
        )
        .await
        .map(drop)
    }
    fn can_prune(&self) -> bool {
        true
    }

    async fn prune(&self, name: &str) -> clax_git::Result<()> {
        call(
            &self.transport,
            request("POST", url("prune", &[("repo", name)]), Vec::new()),
        )
        .await
        .map(drop)
    }
}

impl<T: Transport> Repo for DoRepo<T> {
    async fn read_object(&self, id: &ObjectId) -> clax_git::Result<Option<Object>> {
        let u = url("object", &[("repo", &self.name), ("id", &id.to_hex())]);
        let r = call(&self.transport, request("GET", u, Vec::new())).await?;
        if r.status == 204 {
            return Ok(None);
        }
        let (&code, data) = r.body.split_first().ok_or_else(|| malformed("object"))?;
        let kind = ObjectKind::from_pack_code(code).ok_or_else(|| malformed("object type"))?;
        Ok(Some(Object::new(kind, data.to_vec())))
    }

    async fn refs(&self) -> clax_git::Result<BTreeMap<String, ObjectId>> {
        let r = call(
            &self.transport,
            request("GET", url("refs", &[("repo", &self.name)]), Vec::new()),
        )
        .await?;
        let m: BTreeMap<String, String> =
            serde_json::from_slice(&r.body).map_err(|_| malformed("refs"))?;
        m.into_iter().map(|(k, v)| Ok((k, hex_id(&v)?))).collect()
    }

    async fn apply(
        &self,
        objects: Vec<Object>,
        updates: &[RefUpdate],
        atomic: bool,
    ) -> clax_git::Result<Vec<RefStatus>> {
        let u = url(
            "apply",
            &[
                ("repo", &self.name),
                ("atomic", if atomic { "1" } else { "0" }),
            ],
        );
        let r = call(
            &self.transport,
            request("POST", u, encode_apply(&objects, updates)),
        )
        .await?;
        let v: Vec<Option<String>> =
            serde_json::from_slice(&r.body).map_err(|_| malformed("apply statuses"))?;
        if v.len() != updates.len() {
            return Err(malformed("apply statuses"));
        }
        Ok(v.into_iter().map(|s| s.map_or(Ok(()), Err)).collect())
    }
}

/// Whether a `/git/` request to `path` with `authorization` carries a
/// live token of the repository the path names, asked of `RepoDO`.
pub async fn git_authorized(
    t: &impl Transport,
    path: &str,
    authorization: Option<&str>,
) -> clax_git::Result<bool> {
    let body = json!({ "path": path, "authorization": authorization });
    let r = call(
        t,
        request(
            "POST",
            url("git/authorize", &[]),
            serde_json::to_vec(&body).expect("JSON"),
        ),
    )
    .await?;
    let v: Value = serde_json::from_slice(&r.body).map_err(|_| malformed("authorize"))?;
    Ok(v["access"].is_string())
}

/// clax-core's push ledger ([`clax_core::content::PushLedger`]) over
/// `RepoDO`'s ledger: who pushed, and the repositories' tokens, which
/// `RepoDO` revokes (at Cloudflare Artifacts too, for its tokens).
pub struct DoLedger<T> {
    pub transport: T,
}

fn core(e: GitError) -> clax_core::CoreError {
    clax_content_git::core_error(e)
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<T: Transport + Send + Sync> clax_core::content::PushLedger for DoLedger<T> {
    async fn pushers(
        &self,
        id: &clax_core::ArtifactId,
        commits: &[String],
    ) -> clax_core::Result<Vec<Option<clax_core::content::Pusher>>> {
        let body =
            json!({ "repo": clax_git::versions::repo_name(id.as_str()), "commits": commits });
        let r = call(
            &self.transport,
            request(
                "POST",
                url("push/pushers", &[]),
                serde_json::to_vec(&body).expect("JSON"),
            ),
        )
        .await
        .map_err(core)?;
        let v: Value = serde_json::from_slice(&r.body).map_err(|_| core(malformed("pushers")))?;
        let named = v["pushers"]
            .as_array()
            .ok_or_else(|| core(malformed("pushers")))?;
        Ok(named
            .iter()
            .map(|p| {
                p.is_object().then(|| clax_core::content::Pusher {
                    person: p["person"].as_str().map(String::from),
                    session_id: p["session"].as_str().map(String::from),
                })
            })
            .collect())
    }

    async fn tokens(
        &self,
        id: &clax_core::ArtifactId,
    ) -> clax_core::Result<Vec<clax_core::content::RepoToken>> {
        let body = json!({ "repo": clax_git::versions::repo_name(id.as_str()) });
        let r = call(
            &self.transport,
            request(
                "POST",
                url("token/live", &[]),
                serde_json::to_vec(&body).expect("JSON"),
            ),
        )
        .await
        .map_err(core)?;
        let v: Value = serde_json::from_slice(&r.body).map_err(|_| core(malformed("tokens")))?;
        let rows = v["tokens"]
            .as_array()
            .ok_or_else(|| core(malformed("tokens")))?;
        rows.iter()
            .map(|t| {
                Ok(clax_core::content::RepoToken {
                    id: t["id"]
                        .as_str()
                        .ok_or_else(|| core(malformed("token id")))?
                        .to_string(),
                    write: t["scope"].as_str() == Some(Scope::Write.as_str()),
                    holder: clax_core::content::Pusher {
                        person: t["person"].as_str().map(String::from),
                        session_id: t["session"].as_str().map(String::from),
                    },
                })
            })
            .collect()
    }

    async fn revoke(&self, id: &clax_core::ArtifactId, token: &str) -> clax_core::Result<()> {
        let body = json!({ "repo": clax_git::versions::repo_name(id.as_str()), "id": token });
        call(
            &self.transport,
            request(
                "POST",
                url("token/revoke", &[]),
                serde_json::to_vec(&body).expect("JSON"),
            ),
        )
        .await
        .map(drop)
        .map_err(core)
    }
}

/// Repository tokens, minted and revoked by `RepoDO`.
#[derive(Clone)]
pub struct DoTokens<T> {
    pub transport: T,
}

impl<T: Transport> DoTokens<T> {
    /// A token for `repo` living `ttl_s` seconds, handed to `holder`;
    /// `origin` is the app origin the asking request came to (the
    /// stand-in's remote is served there unless `PUBLIC_ORIGIN` is set).
    pub async fn mint(
        &self,
        repo: &str,
        scope: Scope,
        ttl_s: u32,
        origin: Option<&str>,
        holder: &Holder,
    ) -> clax_git::Result<GitToken> {
        let body = json!({
            "repo": repo,
            "scope": scope.as_str(),
            "ttl_s": ttl_s,
            "origin": origin,
            "person": holder.person,
            "session": holder.session,
            "link": holder.link,
        });
        let r = call(
            &self.transport,
            request(
                "POST",
                url("token/mint", &[]),
                serde_json::to_vec(&body).expect("JSON"),
            ),
        )
        .await?;
        serde_json::from_slice(&r.body).map_err(|_| malformed("token"))
    }

    /// Revokes token `id` of `repo`. A failure at the host leaves it in the
    /// must-revoke list, retried until it is revoked or expires.
    pub async fn revoke(&self, repo: &str, id: &str) -> clax_git::Result<bool> {
        let body = json!({ "repo": repo, "id": id });
        let r = call(
            &self.transport,
            request(
                "POST",
                url("token/revoke", &[]),
                serde_json::to_vec(&body).expect("JSON"),
            ),
        )
        .await?;
        let v: Value = serde_json::from_slice(&r.body).map_err(|_| malformed("revoke"))?;
        v["revoked"].as_bool().ok_or_else(|| malformed("revoke"))
    }

    /// The live tokens handed to a person or an agent session.
    pub async fn held(&self) -> clax_git::Result<Vec<tokens::HeldToken>> {
        let r = call(
            &self.transport,
            request("GET", url("token/held", &[]), Vec::new()),
        )
        .await?;
        serde_json::from_slice(&r.body).map_err(|_| malformed("held tokens"))
    }

    /// Revokes every live token handed to agent session `session` or to
    /// person `person` (their own and their agents'); answers how many.
    pub async fn revoke_held(
        &self,
        person: Option<&str>,
        session: Option<&str>,
    ) -> clax_git::Result<usize> {
        let body = json!({ "person": person, "session": session });
        let r = call(
            &self.transport,
            request(
                "POST",
                url("token/revoke-held", &[]),
                serde_json::to_vec(&body).expect("JSON"),
            ),
        )
        .await?;
        let v: Value = serde_json::from_slice(&r.body).map_err(|_| malformed("revoke"))?;
        v["revoked"]
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| malformed("revoke"))
    }
}
