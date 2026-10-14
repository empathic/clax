//! The stand-in's git smart HTTP at `/git/<repo>.git/…`: clone and fetch
//! with a read or write token, push with a write token, by clax-git's
//! server and its rules (fast forwards only on `refs/heads/*`, `HEAD`'s
//! branch never deleted, Clax's version tags `refs/tags/v<n>` never
//! created, moved or deleted by a push).
//!
//! A pushed commit on `main` is not recorded as a version: the content
//! store refuses the next publish until something records it (record
//! 5fc9c903 on `crates/clax-content-git/src/lib.rs`).

use clax_git::http::HttpResponse;
use clax_git::server::{
    Access, GitRequest, ReceiveOptions, ServeOptions, Served, bearer_token, serve, split_git_path,
    too_large, unauthorized,
};

use super::local::{AsConn, LocalHost};
use super::tokens;

/// One `/git/` request, as the Worker received it.
pub struct GitHttpRequest<'a> {
    pub method: &'a str,
    /// The URL path, `/git/<repo>.git/<rest>`.
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub authorization: Option<&'a str>,
    pub content_encoding: Option<&'a str>,
    pub body: &'a [u8],
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

/// How the stand-in serves: clax-git's defaults, with a push's body and
/// what its pack unpacks to bounded by [`super::db::MAX_WRITE_BYTES`], so a
/// push the database could not take in one transaction is refused before
/// it is read whole or unpacked.
pub fn serve_options() -> ServeOptions {
    let max = super::db::MAX_WRITE_BYTES;
    ServeOptions {
        max_push_bytes: max,
        receive: ReceiveOptions {
            max_unpacked_bytes: max,
            ..ReceiveOptions::default()
        },
        ..ServeOptions::default()
    }
}

/// What the `Authorization` header grants on the repository of `path`:
/// `Ok(None)` when it is no live token of that repository, or `path` is
/// no git path. Checked before a body is read.
pub fn authorize<C: AsConn>(
    host: &LocalHost<C>,
    path: &str,
    authorization: Option<&str>,
    now_ms: i64,
) -> clax_git::Result<Option<Access>> {
    let (Some((repo, _)), Some(token)) =
        (split_git_path(path), authorization.and_then(bearer_token))
    else {
        return Ok(None);
    };
    tokens::check(host, repo, token, now_ms)
}

/// The most body bytes to read for a request to `path` before answering
/// 413 (`None`: not a git path).
pub fn body_limit(path: &str, options: &ServeOptions) -> Option<usize> {
    split_git_path(path).map(|(_, rest)| options.body_limit(rest))
}

/// Answers one `/git/` request on the stand-in's repositories at `now_ms`.
/// A request without a live token of the repository is 401, whether or not
/// the repository exists.
pub async fn serve_git<C: AsConn + clax_git::MaybeSend>(
    host: &LocalHost<C>,
    request: &GitHttpRequest<'_>,
    options: &ServeOptions,
    now_ms: i64,
) -> Served {
    let plain = |response| Served {
        response,
        pushed: Vec::new(),
    };
    let Some((repo, rest)) = split_git_path(request.path) else {
        return plain(text(404, "not a git smart HTTP path\n"));
    };
    let Some(token) = request.authorization.and_then(bearer_token) else {
        return plain(unauthorized());
    };
    let access = match tokens::check(host, repo, token, now_ms) {
        Ok(Some(a)) => a,
        Ok(None) => return plain(unauthorized()),
        Err(e) => return plain(text(503, &format!("{e}\n"))),
    };
    let limit = options.body_limit(rest);
    if request.body.len() > limit {
        return plain(too_large(limit));
    }
    let repo = match host.repo(repo) {
        Ok(Some(r)) => r,
        Ok(None) => return plain(unauthorized()),
        Err(e) => return plain(text(503, &format!("{e}\n"))),
    };
    let git = GitRequest {
        method: request.method,
        rest,
        query: request.query,
        content_encoding: request.content_encoding,
        body: request.body,
    };
    serve(&repo, access, &git, options).await
}
