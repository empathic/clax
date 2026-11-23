//! The first-load data the daemon puts into `index.html` for a hosted
//! daemon's sign-in pages (`/signin`, `/signin/<code>`, `/join/<code>`,
//! `/setup`, `/device`) and its landing page (`/start/`): the answers the
//! page would otherwise fetch once its script runs, so its first render is
//! its settled one. The shell reads it with `readSignInBoot`
//! (`web/shell/src/hosted/signin-boot.ts`, whose `SignInBoot` type is this
//! contract's other half).
//!
//! # The block
//!
//! ```json
//! {"v": 1, "kind": "signin",
//!  "status": {"hosted": 200, "public": 200},
//!  "hosted": {"origin": "…", "signed_in": false, "person": null, …},
//!  "public": {"artifacts": […]},
//!  "invite": null, "link": null, "device": null}
//! ```
//!
//! Each field is the JSON body its route answers this browser, asked as
//! the gallery's block asks its own ([`super::gallery::ask_all`], through
//! the daemon's whole router, as the page load's own `fetch`, within the
//! same budget), and `status.<field>` is that route's status; with no
//! status the field is `null` and the page asks the route itself:
//!
//! - `hosted`, `GET /api/hosted/me`, on every one of these pages.
//! - `public`, `GET /api/hosted/public` (the public gallery), on `/signin`.
//! - `invite`, `GET /api/hosted/invites/<code>`, on `/join/<code>`.
//! - `link`, `GET /api/hosted/signin-links/<code>`, on `/signin/<code>`.
//! - `device`, `GET /api/hosted/device/<code>`, on `/device?code=<code>`
//!   in a browser signed in as a member (the page looks the code up for no
//!   one else).
//!
//! A code the page would send to its route otherwise than as it stands in
//! the address (anything but letters, digits, `-` and `_`) is left out of
//! the block: the page asks for it. The block is left out where the
//! gallery's is ([`super::gallery::answers_as_shell`]).

use super::boot::script_json;
use super::gallery::{Answer, answers_as_shell, ask_all};
use crate::integrations::instance::Instance;
use axum::http::{Extensions, HeaderMap, HeaderValue, Uri};
use clax_http::state::AppState;
use serde_json::{Map, Value, json};

/// A code as it may go into a route's path unchanged: what `encodeURIComponent` leaves alone.
fn plain(code: &str) -> Option<&str> {
    let ok = !code.is_empty()
        && code.len() <= 128
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then_some(code)
}

/// The value of `name` in `query`, decoded.
fn param(query: Option<&str>, name: &str) -> Option<String> {
    url::form_urlencoded::parse(query?.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

/// The fields the page at `path` (with `query`) reads, each with its
/// route; `member`: the browser is signed in as a member, not a guest.
pub fn fields_for(path: &str, query: Option<&str>, member: bool) -> Vec<(&'static str, String)> {
    let mut f = vec![("hosted", "/api/hosted/me".to_string())];
    // The page reads a code as the path names it, less trailing slashes.
    let code = |prefix: &str| {
        path.strip_prefix(prefix)
            .map(|c| c.trim_end_matches('/'))
            .and_then(plain)
            .map(str::to_string)
    };
    if path == "/signin" || path == "/signin/" {
        f.push(("public", "/api/hosted/public".into()));
    } else if let Some(c) = code("/signin/") {
        f.push(("link", format!("/api/hosted/signin-links/{c}")));
    } else if let Some(c) = code("/join/") {
        f.push(("invite", format!("/api/hosted/invites/{c}")));
    } else if path == "/device" && member {
        let c = param(query, "code");
        if let Some(c) = c.as_deref().map(str::trim).and_then(plain) {
            f.push(("device", format!("/api/hosted/device/{c}")));
        }
    }
    f
}

/// The block for the page load at `path` (see the module docs), escaped
/// for its element, and the cookies its routes set; `None` when the load
/// gets none.
pub async fn assemble(
    s: &AppState,
    instance: &Instance,
    uri: &Uri,
    headers: &HeaderMap,
    extensions: &Extensions,
) -> Option<(String, Vec<HeaderValue>)> {
    if !answers_as_shell(headers, uri, extensions) {
        return None;
    }
    let member = extensions
        .get::<clax_http::hosted::HostedRequest>()
        .and_then(|r| r.session.as_ref())
        .is_some_and(|v| !v.guest);
    let fields = fields_for(uri.path(), uri.query(), member);
    let paths: Vec<&str> = fields.iter().map(|(_, p)| p.as_str()).collect();
    let budget = s.gallery_boot_budget.min(s.request_timeout);
    let answers = ask_all(&instance.router(), &paths, headers, extensions, budget).await;
    let mut status = Map::new();
    let mut block = Map::new();
    let mut cookies = Vec::new();
    block.insert("v".into(), json!(1));
    block.insert("kind".into(), json!("signin"));
    for name in ["hosted", "public", "invite", "link", "device"] {
        block.insert(name.into(), Value::Null);
    }
    for ((name, _), a) in fields.iter().zip(answers) {
        let a: Answer = a.unwrap_or_default();
        if let Some(code) = a.status {
            status.insert((*name).into(), json!(code));
        }
        block.insert((*name).into(), a.body);
        cookies.extend(a.cookies);
    }
    block.insert("status".into(), Value::Object(status));
    Some((script_json(&Value::Object(block)), cookies))
}

/// Where `/signin?next=` sends a browser already signed in: `next` when the
/// page would go there itself (`safeNext`, `web/shell/src/hosted/next.ts`:
/// a path of at most 2048 printable ASCII characters that starts with one
/// `/` and holds no backslash), else `None` and the page is served.
pub fn signed_in_next(query: Option<&str>) -> Option<String> {
    let next = param(query, "next")?;
    let ok = !next.is_empty()
        && next.len() <= 2048
        && next.starts_with('/')
        && !next.starts_with("//")
        && !next.contains('\\')
        && next.bytes().all(|b| (0x20..=0x7e).contains(&b));
    ok.then_some(next)
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn each_page_reads_its_own_fields() {
        let names = |p: &str, q: Option<&str>, m: bool| {
            fields_for(p, q, m)
                .into_iter()
                .map(|(n, r)| format!("{n} {r}"))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names("/signin", None, false),
            ["hosted /api/hosted/me", "public /api/hosted/public"]
        );
        assert_eq!(
            names("/signin/AbC-12_x", None, false),
            [
                "hosted /api/hosted/me",
                "link /api/hosted/signin-links/AbC-12_x"
            ]
        );
        assert_eq!(
            names("/signin/AbC//", None, false),
            ["hosted /api/hosted/me", "link /api/hosted/signin-links/AbC"]
        );
        assert_eq!(
            names("/join/k9", None, true),
            ["hosted /api/hosted/me", "invite /api/hosted/invites/k9"]
        );
        assert_eq!(
            names("/device", Some("code=WXYZ-4821"), true),
            [
                "hosted /api/hosted/me",
                "device /api/hosted/device/WXYZ-4821"
            ]
        );
        assert_eq!(
            names("/device", Some("code=+WXYZ-4821+"), true),
            [
                "hosted /api/hosted/me",
                "device /api/hosted/device/WXYZ-4821"
            ]
        );
        assert_eq!(
            names("/setup", Some("secret=s"), false),
            ["hosted /api/hosted/me"]
        );
        assert_eq!(names("/start/", None, false), ["hosted /api/hosted/me"]);
    }

    #[test]
    fn a_code_the_page_would_encode_or_would_not_look_up_is_left_to_the_page() {
        for (p, q, m) in [
            ("/join/a%20b", None, true),
            ("/signin/a.b", None, true),
            ("/join/", None, true),
            ("/device", Some("code=WXYZ-4821"), false),
            ("/device", Some("code=a%2Fb"), true),
            ("/device", None, true),
        ] {
            assert_eq!(fields_for(p, q, m).len(), 1, "{p} {q:?} {m}");
        }
    }

    #[test]
    fn next_is_taken_only_where_the_page_would_go_there() {
        assert_eq!(
            signed_in_next(Some("next=%2Ftags")).as_deref(),
            Some("/tags")
        );
        assert_eq!(
            signed_in_next(Some("a=1&next=/site-signin%3Fhost%3Dx")).as_deref(),
            Some("/site-signin?host=x")
        );
        assert_eq!(signed_in_next(Some("next=/")).as_deref(), Some("/"));
        for q in [
            "",
            "next=",
            "next=https%3A%2F%2Fevil",
            "next=%2F%2Fevil",
            "next=%2F%5Cevil",
            "next=%2Fa%09b",
            "next=%2F%C3%A9",
            "next=tags",
        ] {
            assert_eq!(signed_in_next(Some(q)), None, "{q}");
        }
        assert_eq!(
            signed_in_next(Some(&format!("next=/{}", "a".repeat(2048)))),
            None
        );
    }
}
